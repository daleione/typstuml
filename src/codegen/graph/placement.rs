use super::geom::NodeGeom;
use crate::layout::geometry::Point;

pub(crate) fn violates_containment(
    ei: usize,
    new_box: (Point, Point),
    entity_container: &[Option<usize>],
    container_bboxes: &[Option<(Point, Point)>],
) -> bool {
    let own = entity_container[ei];
    container_bboxes.iter().enumerate().any(|(ci, bb)| {
        let Some((bx0, bx1)) = bb else {
            return false;
        };
        let overlaps = new_box.0.x < bx1.x
            && bx0.x < new_box.1.x
            && new_box.0.y < bx1.y
            && bx0.y < new_box.1.y;
        if Some(ci) == own {
            // Must stay fully inside its own frame.
            !(new_box.0.x >= bx0.x
                && new_box.0.y >= bx0.y
                && new_box.1.x <= bx1.x
                && new_box.1.y <= bx1.y)
        } else {
            overlaps
        }
    })
}

pub(crate) fn recenter(
    top_lefts: &mut [Point],
    geoms: &[NodeGeom],
    layout_edges: &[(usize, usize)],
    entity_container: &[Option<usize>],
    container_bboxes: &[Option<(Point, Point)>],
) {
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); geoms.len()];
    for &(s, d) in layout_edges {
        preds[d].push(s);
    }
    let entity_count = geoms.len();
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
        // Reject if the move would crash into another entity.
        // Bbox overlap is the real test — a same-rank y-proximity
        // gate used to stand in for it here, but in a multi-cluster
        // layout two entities at the same DAG rank can have
        // slightly different absolute y (different ancestor pad /
        // label-band), so a small but genuine y-gap could slip
        // past a coarse threshold while the bboxes still overlap.
        let conflict = (0..entity_count).any(|j| {
            if j == ei {
                return false;
            }
            let other = (top_lefts[j], top_lefts[j].add(geoms[j].size));
            new_box.0.x < other.1.x
                && other.0.x < new_box.1.x
                && new_box.0.y < other.1.y
                && other.0.y < new_box.1.y
        });
        if !conflict && !violates_containment(ei, new_box, entity_container, container_bboxes) {
            top_lefts[ei] = Point::new(new_x, my_y);
        }
    }
}
