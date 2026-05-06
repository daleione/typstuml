//! Initial Y / X assignment: pack each rank tightly along its row.

use super::EPSILON;
use crate::layout::geometry::Point;
use crate::layout::graph::VisualGraph;

/// Translate the whole graph so the leftmost box edge sits at x = 0.
pub(crate) fn align_to_left(vg: &mut VisualGraph) {
    let mut first_x = f64::INFINITY;
    for elem in vg.iter_nodes() {
        first_x = first_x.min(vg.pos(elem).bbox(true).0.x);
    }
    if first_x.is_infinite() {
        return;
    }
    for elem in vg.iter_nodes() {
        vg.pos_mut(elem).translate(Point::new(-first_x, 0.));
    }
}

fn assign_y_coordinates(vg: &mut VisualGraph) {
    let mut top = 0.;
    for i in 0..vg.dag.num_levels() {
        let row = vg.dag.row(i).clone();
        let max_height = row
            .iter()
            .map(|n| vg.pos(*n).size(true).y)
            .fold(0., f64::max);
        let center = top + max_height / 2.;
        for n in &row {
            let h = vg.pos(*n).size(true).y;
            vg.pos_mut(*n).align_to_top(center - h / 2.);
        }
        top += max_height;
    }
}

fn assign_x_coordinates(vg: &mut VisualGraph) {
    for i in 0..vg.dag.num_levels() {
        let row = vg.dag.row(i).clone();
        let mut right = 0.;
        for n in &row {
            let pos = vg.pos_mut(*n);
            pos.align_to_left(right + EPSILON);
            right = pos.bbox(true).1.x + EPSILON;
        }
    }
}

pub(crate) fn do_it(vg: &mut VisualGraph) {
    assign_y_coordinates(vg);
    assign_x_coordinates(vg);
}
