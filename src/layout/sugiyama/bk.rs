//! Brandes-Kopf x-coordinate assignment.
//!
//! Reference: U. Brandes and B. Köpf, "Fast and Simple Horizontal
//! Coordinate Assignment", Graph Drawing 2001. We run all four corner
//! sweeps (LR/RL × top/bottom alignment), then average the resulting x
//! placements per node — this balances out the bias of any single sweep.

use std::collections::HashSet;

use super::simple;
use crate::layout::dag::NodeHandle;
use crate::layout::geometry::weighted_median;
use crate::layout::graph::VisualGraph;

#[derive(Debug, Clone, Copy)]
enum OrderLR {
    LeftToRight,
    RightToLeft,
}

impl OrderLR {
    fn is_left_to_right(&self) -> bool {
        matches!(self, OrderLR::LeftToRight)
    }
}

/// For each node: which node above it is the head of its vertical block,
/// and which node below it (if any) is glued onto it.
struct NodeAttachInfo {
    above: Vec<Option<NodeHandle>>,
    below: Vec<Option<NodeHandle>>,
}

impl NodeAttachInfo {
    fn new(size: usize) -> Self {
        Self {
            above: vec![None; size],
            below: vec![None; size],
        }
    }

    fn add(&mut self, from: NodeHandle, to: NodeHandle) {
        debug_assert!(self.below[to.get_index()].is_none(), "target already taken");
        debug_assert!(self.above[from.get_index()].is_none(), "source already set");
        self.above[from.get_index()] = Some(to);
        self.below[to.get_index()] = Some(from);
    }

    /// Walk the alignment chain into vertical blocks, each a list of nodes
    /// from bottom to top.
    fn into_verticals(self) -> VerticalList {
        let mut res = VerticalList::new();
        let mut used = vec![false; self.above.len()];
        for i in 0..self.above.len() {
            if used[i] {
                continue;
            }
            let mut idx = i;
            while let Some(below) = self.below[idx] {
                idx = below.get_index();
            }
            let mut vertical = vec![NodeHandle::from(idx)];
            while let Some(above) = self.above[idx] {
                if used[idx] {
                    break;
                }
                used[idx] = true;
                idx = above.get_index();
                vertical.push(NodeHandle::from(idx));
            }
            used[idx] = true;
            res.push(vertical);
        }
        res
    }
}

type EdgeSet = HashSet<(NodeHandle, NodeHandle)>;
type EdgeIdxs = (usize, usize);
type Vertical = Vec<NodeHandle>;
type VerticalList = Vec<Vertical>;

/// Schedules verticals one at a time, each at the leftmost (or rightmost)
/// x where every node in it still fits within the row spacing.
struct Scheduler<'a> {
    vg: &'a VisualGraph,
    vl: VerticalList,
    x_coords: Vec<f64>,
    sched_idx: Vec<usize>,
    last_x_for_row: Vec<f64>,
    order: OrderLR,
}

impl<'a> Scheduler<'a> {
    fn new(vg: &'a VisualGraph, vl: VerticalList, order: OrderLR) -> Self {
        let bound = if order.is_left_to_right() {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        Self {
            vg,
            vl,
            x_coords: vec![0.; vg.num_nodes()],
            sched_idx: vec![0; vg.dag.num_levels()],
            last_x_for_row: vec![bound; vg.dag.num_levels()],
            order,
        }
    }

    fn into_x_placement(self) -> Vec<f64> {
        self.x_coords
    }

    fn schedule(&mut self) {
        let mut to_place = self.vl.len();
        while to_place > 0 {
            for i in 0..self.vl.len() {
                if !self.is_vertical_ready(i) {
                    continue;
                }
                let x = self.first_schedule_x(&self.vl[i]);
                self.place_vertical(i, x);
                self.vl[i].clear();
                to_place -= 1;
            }
        }
    }

    fn first_schedule_x(&self, v: &Vertical) -> f64 {
        let mut x: f64 = 0.;
        for elem in v {
            let level = self.vg.dag.level(*elem);
            let last = self.last_x_for_row[level];
            let pos = self.vg.pos(*elem);
            if self.order.is_left_to_right() {
                x = x.max(last + pos.distance_to_left(true));
            } else {
                x = x.min(last - pos.distance_to_right(true));
            }
        }
        x
    }

    fn place_vertical(&mut self, i: usize, center_x: f64) {
        let v = &self.vl[i];
        for elem in v {
            self.x_coords[elem.get_index()] = center_x;
            let level = self.vg.dag.level(*elem);
            let pos = self.vg.pos(*elem);
            self.last_x_for_row[level] = if self.order.is_left_to_right() {
                center_x + pos.distance_to_right(true)
            } else {
                center_x - pos.distance_to_left(true)
            };
            self.sched_idx[level] += 1;
        }
    }

    fn is_next_avail_in_row(&self, node: NodeHandle, row_idx: usize) -> bool {
        let row = self.vg.dag.row(row_idx);
        let first_free = self.sched_idx[row_idx];
        if first_free >= row.len() {
            return false;
        }
        let target = if self.order.is_left_to_right() {
            row[first_free]
        } else {
            row[row.len() - first_free - 1]
        };
        target == node
    }

    fn is_vertical_ready(&self, idx: usize) -> bool {
        let v = &self.vl[idx];
        if v.is_empty() {
            return false;
        }
        v.iter()
            .all(|n| self.is_next_avail_in_row(*n, self.vg.dag.level(*n)))
    }
}

pub struct BK<'a> {
    vg: &'a mut VisualGraph,
}

impl<'a> BK<'a> {
    pub fn new(vg: &'a mut VisualGraph) -> Self {
        Self { vg }
    }

    /// Two row-to-row edges cross when their endpoint orderings disagree.
    fn are_edges_crossing(a: EdgeIdxs, b: EdgeIdxs) -> bool {
        let parallel = (a.0 < b.0 && a.1 < b.1) || (a.0 > b.0 && a.1 > b.1);
        !parallel
    }

    /// Edges that don't conflict with any "type-2" (connector-to-connector)
    /// crossing — those are the edges BK is allowed to align along.
    fn get_valid_edges(&self) -> EdgeSet {
        let mut out = EdgeSet::new();
        for i in 0..self.vg.dag.num_levels().saturating_sub(1) {
            let r0 = self.vg.dag.row(i);
            let r1 = self.vg.dag.row(i + 1);
            for e in self.extract_safe_edges(r0, r1) {
                out.insert(e);
            }
        }
        out
    }

    fn extract_safe_edges(
        &self,
        r0: &[NodeHandle],
        r1: &[NodeHandle],
    ) -> Vec<(NodeHandle, NodeHandle)> {
        let mut regular: Vec<EdgeIdxs> = Vec::new();
        let mut strong: Vec<EdgeIdxs> = Vec::new();
        for (i0, elem) in r0.iter().enumerate() {
            for succ in self.vg.succ(*elem) {
                let i1 = match r1.iter().position(|r| r == succ) {
                    Some(p) => p,
                    None => continue,
                };
                let both_connector = self.vg.is_connector(*elem) && self.vg.is_connector(*succ);
                if both_connector {
                    strong.push((i0, i1));
                } else {
                    regular.push((i0, i1));
                }
            }
        }
        let mut out = Vec::new();
        'outer: for reg in &regular {
            for s in &strong {
                if !Self::are_edges_crossing(*reg, *s) {
                    continue;
                }
                continue 'outer;
            }
            out.push((r0[reg.0], r1[reg.1]));
        }
        for s in strong {
            out.push((r0[s.0], r1[s.1]));
        }
        out
    }

    fn get_pred_medians(&self, valid: &EdgeSet) -> Vec<f64> {
        let mut res = Vec::with_capacity(self.vg.num_nodes());
        let mut buf: Vec<f64> = Vec::new();
        for node in self.vg.iter_nodes() {
            buf.clear();
            for pred in self.vg.preds(node) {
                if !valid.contains(&(*pred, node)) {
                    continue;
                }
                buf.push(self.vg.pos(*pred).center().x);
            }
            res.push(if buf.is_empty() {
                0.
            } else {
                weighted_median(&buf)
            });
        }
        res
    }

    fn compute_alignment(&self, order: OrderLR) -> NodeAttachInfo {
        let mut align = NodeAttachInfo::new(self.vg.num_nodes());
        let valid = self.get_valid_edges();
        let medians = self.get_pred_medians(&valid);

        for i in 0..self.vg.dag.num_levels().saturating_sub(1) {
            let mut r0 = self.vg.dag.row(i).clone();
            let mut r1 = self.vg.dag.row(i + 1).clone();
            let mut used = vec![false; r0.len()];
            if !order.is_left_to_right() {
                r0.reverse();
                r1.reverse();
            }

            for node in r1 {
                let target = medians[node.get_index()];
                let mut best: Option<usize> = None;
                let mut best_delta = f64::INFINITY;
                for pred in self.vg.preds(node) {
                    let idx = match r0.iter().position(|p| p == pred) {
                        Some(i) => i,
                        None => continue,
                    };
                    if used[idx] {
                        continue;
                    }
                    let delta = (self.vg.pos(*pred).center().x - target).abs();
                    if delta < best_delta {
                        best = Some(idx);
                        best_delta = delta;
                    }
                }
                if let Some(idx) = best {
                    for u in &mut used[..=idx] {
                        *u = true;
                    }
                    align.add(node, r0[idx]);
                }
            }
        }
        align
    }

    fn run_pass(&self, alignment: OrderLR, schedule: OrderLR) -> Vec<f64> {
        let vl = self.compute_alignment(alignment).into_verticals();
        let mut sc = Scheduler::new(self.vg, vl, schedule);
        sc.schedule();
        sc.into_x_placement()
    }

    pub fn do_it(&mut self) {
        // Four corner sweeps: each pairs an alignment direction with a
        // scheduling direction. Averaging the four x's balances the bias
        // each sweep introduces toward its corner.
        let xs0 = self.run_pass(OrderLR::RightToLeft, OrderLR::RightToLeft);
        let xs1 = self.run_pass(OrderLR::RightToLeft, OrderLR::LeftToRight);
        let xs2 = self.run_pass(OrderLR::LeftToRight, OrderLR::RightToLeft);
        let xs3 = self.run_pass(OrderLR::LeftToRight, OrderLR::LeftToRight);

        for i in 0..xs0.len() {
            let val = (xs0[i] + xs1[i] + xs2[i] + xs3[i]) / 4.0;
            self.vg.pos_mut(NodeHandle::from(i)).set_x(val);
        }
        simple::align_to_left(self.vg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_crossing() {
        assert!(BK::are_edges_crossing((0, 10), (10, 0)));
        assert!(!BK::are_edges_crossing((0, 0), (10, 10)));
        assert!(!BK::are_edges_crossing((10, 0), (13, 3)));
        assert!(BK::are_edges_crossing((10, 0), (0, 10)));
        assert!(BK::are_edges_crossing((0, 10), (13, 10)));
        assert!(!BK::are_edges_crossing((0, 10), (13, 11)));
    }

    #[test]
    fn extract_verticals_chains() {
        let mut ai = NodeAttachInfo::new(6);
        ai.add(NodeHandle::new(0), NodeHandle::new(1));
        ai.add(NodeHandle::new(1), NodeHandle::new(2));
        ai.add(NodeHandle::new(2), NodeHandle::new(3));
        ai.add(NodeHandle::new(4), NodeHandle::new(5));
        let v = ai.into_verticals();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], (0..=3).map(NodeHandle::new).collect::<Vec<_>>());
        assert_eq!(v[1], vec![NodeHandle::new(4), NodeHandle::new(5)]);
    }
}
