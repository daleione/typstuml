//! Network-simplex x-coordinate assignment (Gansner et al. §4.2,
//! graphviz `lib/dotgen/position.c`).
//!
//! Replaces Brandes-Köpf for dot-style diagrams. Builds the auxiliary
//! graph and runs [`ns::solve`]; the resulting rank of each real node is
//! its x-coordinate (the perpendicular axis in the internal top-to-bottom
//! frame). Because it globally minimises weighted edge length, chains
//! come out straight and a parent centres over its children — so the
//! per-diagram perp-centering / composite-offset patches are unnecessary.

use super::{ns, simple};
use crate::layout::dag::NodeHandle;
use crate::layout::graph::VisualGraph;

/// Edge-straightening weight Ω: heavier for virtual (connector) chains so
/// long edges run straight, exactly as dot weights real/virtual segments.
fn omega(vg: &VisualGraph, u: NodeHandle, v: NodeHandle) -> f64 {
    match (vg.is_connector(u), vg.is_connector(v)) {
        (false, false) => 1.0,
        (true, true) => 8.0,
        _ => 2.0,
    }
}

pub(crate) fn do_it(vg: &mut VisualGraph) {
    let n = vg.num_nodes();
    if n == 0 {
        return;
    }
    let mut edges: Vec<(usize, usize, f64, f64)> = Vec::new();
    // Edge-node pairs: for every adjacent-rank graph edge u→v, a slack
    // node `en` with en→u and en→v (minlen 0, weight Ω). Minimising their
    // length pulls u and v onto the same x — i.e. straightens the edge.
    let mut next = n;
    for u in 0..n {
        let uh = NodeHandle::from(u);
        for &v in vg.dag.successors(uh) {
            let w = omega(vg, uh, v);
            let en = next;
            next += 1;
            edges.push((en, u, 0.0, w));
            edges.push((en, v.get_index(), 0.0, w));
        }
    }
    // Separation constraints: adjacent nodes in a rank must keep their
    // half-widths (halo-inclusive) apart, in row order. Weight 0 — pure
    // constraint.
    for r in 0..vg.dag.num_levels() {
        let row = vg.dag.row(r);
        for pair in row.windows(2) {
            let (l, rt) = (pair[0], pair[1]);
            let sep = vg.pos(l).size(true).x / 2.0 + vg.pos(rt).size(true).x / 2.0;
            edges.push((l.get_index(), rt.get_index(), sep, 0.0));
        }
    }

    let x = ns::solve(next, &edges);
    for u in 0..n {
        vg.pos_mut(NodeHandle::from(u)).set_x(x[u]);
    }
    simple::align_to_left(vg);
}
