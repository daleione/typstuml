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

use crate::layout::geometry::Point;
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
}
