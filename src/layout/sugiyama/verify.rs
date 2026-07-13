//! Sanity check: after placement, the rendered boxes in a rank must not
//! overlap and must be ordered left-to-right. `bbox(false)` is the actual
//! geometry; the halo around it is reservation, not an overlap.

use crate::layout::geometry::do_boxes_intersect;
use crate::layout::graph::VisualGraph;

pub(crate) fn do_it(vg: &mut VisualGraph) {
    for row_idx in 0..vg.dag.num_levels() {
        let row = vg.dag.row(row_idx);
        for window in row.windows(2) {
            let bb0 = vg.pos(window[0]).bbox(false);
            let bb1 = vg.pos(window[1]).bbox(false);
            assert!(!do_boxes_intersect(bb0, bb1), "boxes must not intersect");
            assert!(
                bb0.0.x < bb1.0.x,
                "boxes must be ordered left-to-right within a rank"
            );
        }
    }
}

/// Global containment/overlap safety net for hierarchical (cuca) layouts,
/// run once after `tighten::do_it` has settled every cluster frame. A
/// no-op for graphs with no hierarchy. Turns a silent visual regression
/// (an entity drawn under a foreign package frame, or two entities
/// overlapping across ranks) into a panic during `cargo test` / debug
/// builds — the executable form of the containment contract described
/// in `docs/cuca-architecture-layout-redesign.md` §3.2c.
pub(crate) fn verify_final(vg: &VisualGraph) {
    if vg.hierarchy.is_empty() {
        return;
    }
    let real: Vec<_> = vg.iter_nodes().filter(|h| !vg.is_connector(*h)).collect();
    for i in 0..real.len() {
        for j in (i + 1)..real.len() {
            let a = vg.pos(real[i]).bbox(false);
            let b = vg.pos(real[j]).bbox(false);
            assert!(
                !do_boxes_intersect(a, b),
                "entities must not overlap after tighten: {a:?} vs {b:?}"
            );
        }
    }
    for c in 0..vg.hierarchy.clusters.len() {
        let cl = &vg.hierarchy.clusters[c];
        if !cl.x_min.is_finite() {
            continue;
        }
        let frame = (
            crate::layout::geometry::Point::new(cl.x_min, cl.y_min),
            crate::layout::geometry::Point::new(cl.x_max, cl.y_max),
        );
        for &n in &real {
            if vg.hierarchy.is_inside(n, c) {
                continue;
            }
            let nb = vg.pos(n).bbox(false);
            assert!(
                !do_boxes_intersect(nb, frame),
                "entity {nb:?} must not intersect foreign cluster frame {frame:?}"
            );
        }
    }
}
