//! Bezier path extraction for a routed edge.
//!
//! After `VisualGraph::layout()`, an edge is a chain of placed elements:
//! `[from, ...connectors, to]`. We turn that into a sequence of
//! `(start, control)` pairs the painter consumes — for an n-element chain,
//! there are n-1 cubic segments and the consumer pairs them up in order.

use crate::layout::geometry::Point;
use crate::layout::graph::Element;

pub fn generate_curve(elements: &[Element], force: f64) -> Vec<(Point, Point)> {
    debug_assert!(elements.len() >= 2, "need at least source and sink");

    let mut path = Vec::with_capacity(elements.len());
    let next = elements[1].position().center();
    let from_con = elements[0].connector_location(next, force);
    let mut prev_exit = from_con.0;
    path.push(from_con);

    for i in 1..elements.len() {
        let con = if i == elements.len() - 1 {
            elements[i].connector_location(prev_exit, force)
        } else {
            let next_loc = elements[i + 1].position().center();
            elements[i].passthrough_control(prev_exit, next_loc, force)
        };
        prev_exit = con.0;
        path.push((con.1, con.0));
    }

    path
}
