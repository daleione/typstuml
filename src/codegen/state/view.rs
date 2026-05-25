//! Read-only views over a `StateDiagram` and its laid-out node boxes,
//! shared by the router (`route`) and the emitter (`emit`).
//!
//! `NodeTopology` answers id / parent / ancestor queries; `NodeBoxes` answers
//! geometry queries (centre, bbox, perimeter clip) over the absolute
//! coordinates produced by the layout. Both replace the per-function closures
//! that previously rebuilt the same id→index map and centre arithmetic.

use std::collections::HashMap;

use crate::ir::StateDiagram;
use crate::layout::geometry::Point;

use super::route::{node_shape, perimeter_point};

/// Node id→index map plus the parent chain, with ancestor queries.
pub(super) struct NodeTopology<'a> {
    idx: HashMap<&'a str, usize>,
    parent_of: Vec<Option<usize>>,
}

impl<'a> NodeTopology<'a> {
    pub(super) fn new(diag: &'a StateDiagram) -> Self {
        let idx: HashMap<&str, usize> = diag
            .nodes
            .iter()
            .enumerate()
            .map(|(i, nd)| (nd.id.as_str(), i))
            .collect();
        let parent_of = diag
            .nodes
            .iter()
            .map(|nd| nd.parent.as_deref().and_then(|p| idx.get(p).copied()))
            .collect();
        Self { idx, parent_of }
    }

    /// Resolve a node id to its index.
    pub(super) fn index(&self, id: &str) -> Option<usize> {
        self.idx.get(id).copied()
    }

    /// `node`'s parent index, if any.
    pub(super) fn parent(&self, node: usize) -> Option<usize> {
        self.parent_of[node]
    }

    /// True iff `a` is an ancestor-or-self of `x` (walking x's parent chain).
    pub(super) fn anc_or_self(&self, a: usize, mut x: usize) -> bool {
        loop {
            if x == a {
                return true;
            }
            match self.parent_of[x] {
                Some(p) => x = p,
                None => return false,
            }
        }
    }
}

/// Geometry view over the laid-out node boxes: `top_lefts[i]` is the absolute
/// top-left and `eff_geom[i]` the size of node `i`.
pub(super) struct NodeBoxes<'a> {
    diag: &'a StateDiagram,
    top_lefts: &'a [Point],
    eff_geom: &'a [Point],
}

impl<'a> NodeBoxes<'a> {
    pub(super) fn new(
        diag: &'a StateDiagram,
        top_lefts: &'a [Point],
        eff_geom: &'a [Point],
    ) -> Self {
        Self {
            diag,
            top_lefts,
            eff_geom,
        }
    }

    /// Centre of node `i`.
    pub(super) fn center(&self, i: usize) -> Point {
        Point::new(
            self.top_lefts[i].x + self.eff_geom[i].x / 2.0,
            self.top_lefts[i].y + self.eff_geom[i].y / 2.0,
        )
    }

    /// `(lo, hi)` corner points of node `i`'s bbox.
    pub(super) fn bbox(&self, i: usize) -> (Point, Point) {
        let lo = self.top_lefts[i];
        (
            lo,
            Point::new(lo.x + self.eff_geom[i].x, lo.y + self.eff_geom[i].y),
        )
    }

    /// Point on node `i`'s perimeter along the ray toward `toward`.
    pub(super) fn perimeter_toward(&self, i: usize, toward: Point) -> Point {
        perimeter_point(
            self.center(i),
            self.eff_geom[i].x / 2.0,
            self.eff_geom[i].y / 2.0,
            node_shape(self.diag.nodes[i].kind),
            toward,
        )
    }
}
