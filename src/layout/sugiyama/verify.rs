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
