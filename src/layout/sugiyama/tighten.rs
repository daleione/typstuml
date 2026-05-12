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

use crate::layout::geometry::Point;
use crate::layout::graph::VisualGraph;
use crate::layout::sugiyama::hierarchy::ClusterId;

/// Minimum gap enforced between two sibling clusters' outer bboxes
/// along the row axis. Matches the painter's visual rhythm.
const CLUSTER_GAP_PT: f64 = 12.0;

/// Run tighten on every cluster. No-op when the hierarchy is empty.
pub(crate) fn do_it(vg: &mut VisualGraph) {
    if vg.hierarchy.is_empty() {
        return;
    }
    compute_bboxes(vg);
    resolve_sibling_overlap(vg);
}

fn compute_bboxes(vg: &mut VisualGraph) {
    // Snapshot direct_nodes / direct_children to a local clone so we
    // can mutate cluster bbox fields without aliasing.
    let n = vg.hierarchy.clusters.len();
    let direct_nodes: Vec<Vec<_>> = (0..n)
        .map(|i| vg.hierarchy.clusters[i].direct_nodes.clone())
        .collect();
    let direct_children: Vec<Vec<ClusterId>> = (0..n)
        .map(|i| vg.hierarchy.clusters[i].direct_children.clone())
        .collect();
    let pads: Vec<f64> = (0..n).map(|i| vg.hierarchy.clusters[i].pad).collect();
    let label_bands: Vec<f64> = (0..n)
        .map(|i| vg.hierarchy.clusters[i].label_band)
        .collect();
    let label_min_ws: Vec<f64> = (0..n)
        .map(|i| vg.hierarchy.clusters[i].label_min_w)
        .collect();

    let order = post_order(&direct_children, n);

    for c in order {
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;

        for &h in &direct_nodes[c] {
            // Skip connector dummies — `assign_node` only registers real
            // entities, but `inherit_node` does NOT add to direct_nodes,
            // so this loop already only sees real entities. The is_connector
            // check is defensive in case a future caller pushes a connector
            // into direct_nodes by mistake.
            if vg.is_connector(h) {
                continue;
            }
            let (tl, br) = vg.pos(h).bbox(false);
            x_min = x_min.min(tl.x);
            y_min = y_min.min(tl.y);
            x_max = x_max.max(br.x);
            y_max = y_max.max(br.y);
        }
        for &child in &direct_children[c] {
            let child_cluster = &vg.hierarchy.clusters[child];
            if child_cluster.x_min.is_finite() {
                x_min = x_min.min(child_cluster.x_min);
                y_min = y_min.min(child_cluster.y_min);
                x_max = x_max.max(child_cluster.x_max);
                y_max = y_max.max(child_cluster.y_max);
            }
        }
        if !x_min.is_finite() {
            // Empty cluster: leave bbox at sentinel infinity so codegen
            // can detect and skip it.
            continue;
        }
        let pad = pads[c];
        let band = label_bands[c];
        let min_w = label_min_ws[c];
        let outer_w = (x_max - x_min + 2.0 * pad).max(min_w);
        let cluster = &mut vg.hierarchy.clusters[c];
        cluster.x_min = x_min - pad;
        cluster.x_max = cluster.x_min + outer_w;
        cluster.y_min = y_min - pad - band;
        cluster.y_max = y_max + pad;
    }
}

/// Walk top-down through cluster levels. At each level (siblings under
/// the same parent, plus top-level roots), if two sibling clusters'
/// outer bboxes overlap horizontally, shift the right-hand cluster (and
/// its subtree) far enough to clear with a `CLUSTER_GAP_PT` gap.
///
/// BK places nodes without knowing about cluster padding, so two
/// clusters' members can sit closer together than their *cluster*
/// bboxes allow. This pass enforces the cluster-level gap after the
/// fact; bboxes were already computed in `compute_bboxes`, so we just
/// translate.
fn resolve_sibling_overlap(vg: &mut VisualGraph) {
    let n = vg.hierarchy.clusters.len();

    // Roots = clusters with no parent. Discovered by scanning `parent`.
    let mut roots: Vec<ClusterId> = Vec::new();
    for i in 0..n {
        if vg.hierarchy.clusters[i].parent.is_none() {
            roots.push(i);
        }
    }

    let mut queue: Vec<Vec<ClusterId>> = vec![roots];
    while let Some(level) = queue.pop() {
        // Sort siblings by current x_min so we can sweep left-to-right.
        // Clusters with infinite x_min (empty) are dropped — nothing to
        // shift.
        let mut sorted: Vec<ClusterId> = level
            .iter()
            .copied()
            .filter(|&c| vg.hierarchy.clusters[c].x_min.is_finite())
            .collect();
        sorted.sort_by(|a, b| {
            vg.hierarchy.clusters[*a]
                .x_min
                .partial_cmp(&vg.hierarchy.clusters[*b].x_min)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Pairwise overlap resolution: each cluster's bbox may overlap
        // with multiple earlier siblings. For each pair, check both
        // axes; if both overlap, shift along whichever axis has the
        // smaller required correction — pads alone usually cause
        // overlap in one direction while the entity layout already
        // separates the clusters in the other.
        for i in 1..sorted.len() {
            let cur = sorted[i];
            // Recompute overlap against every earlier sibling because
            // `cur` may already have been shifted by an earlier pass.
            for j in 0..i {
                let prev = sorted[j];
                let p = &vg.hierarchy.clusters[prev];
                let c = &vg.hierarchy.clusters[cur];
                let x_overlap = p.x_max - c.x_min;
                let y_overlap = p.y_max - c.y_min;
                if x_overlap <= 0.0 || y_overlap <= 0.0 {
                    continue;
                }
                let dx = x_overlap + CLUSTER_GAP_PT;
                let dy = y_overlap + CLUSTER_GAP_PT;
                if dx <= dy {
                    shift_cluster_subtree(vg, cur, dx, 0.0);
                } else {
                    shift_cluster_subtree(vg, cur, 0.0, dy);
                }
            }
        }

        // Recurse into the next level: every direct child of every
        // cluster on this level.
        for &c in &level {
            let children = vg.hierarchy.clusters[c].direct_children.clone();
            if !children.is_empty() {
                queue.push(children);
            }
        }
    }
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

/// Post-order (children before parents) traversal of the cluster forest.
fn post_order(direct_children: &[Vec<ClusterId>], n: usize) -> Vec<ClusterId> {
    let mut visited = vec![false; n];
    let mut out = Vec::with_capacity(n);
    let mut roots: Vec<ClusterId> = Vec::new();
    let mut is_child = vec![false; n];
    for kids in direct_children {
        for &k in kids {
            is_child[k] = true;
        }
    }
    for i in 0..n {
        if !is_child[i] {
            roots.push(i);
        }
    }
    // Iterative DFS to avoid stack blow-up on pathological depth.
    let mut stack: Vec<(ClusterId, usize)> = roots.iter().map(|&r| (r, 0)).collect();
    while let Some((c, child_idx)) = stack.pop() {
        if child_idx < direct_children[c].len() {
            stack.push((c, child_idx + 1));
            let next = direct_children[c][child_idx];
            if !visited[next] {
                stack.push((next, 0));
            }
        } else if !visited[c] {
            visited[c] = true;
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_order_visits_children_first() {
        // 0
        // └─ 1
        //    └─ 2
        let children = vec![vec![1], vec![2], vec![]];
        let order = post_order(&children, 3);
        assert_eq!(order, vec![2, 1, 0]);
    }

    #[test]
    fn post_order_handles_forest() {
        // 0 -> 1, 2 -> 3
        let children = vec![vec![1], vec![], vec![3], vec![]];
        let order = post_order(&children, 4);
        // Both subtrees: children before roots.
        assert!(order.iter().position(|&x| x == 1) < order.iter().position(|&x| x == 0));
        assert!(order.iter().position(|&x| x == 3) < order.iter().position(|&x| x == 2));
        assert_eq!(order.len(), 4);
    }
}
