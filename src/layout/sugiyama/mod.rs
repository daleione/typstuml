//! Sugiyama-style layered layout. Drives the four standard phases against
//! a `VisualGraph` whose DAG and node sizes are already populated:
//!
//! 1. Optional rank reduction (sink nodes to shorten long edges).
//! 2. Optional crossing reduction (reorder nodes within each rank).
//! 3. X assignment via Brandes-Kopf, averaging four passes.
//! 4. Edge straightening / overlap fixups.
//!
//! Steps 1 and 2 live alongside the `Placer` here; the per-step machinery
//! lives in submodules.

mod bk;
mod compact;
mod edge_fix;
pub mod hierarchy;
mod port_align;
mod simple;
pub mod tighten;
mod verify;

pub use hierarchy::{ClusterId, HCluster, HierarchyMap};

use crate::layout::dag::{NodeHandle, DAG};
use crate::layout::graph::VisualGraph;

/// Tolerance used when sweeping consecutive boxes along an axis. The exact
/// value isn't load-bearing — it just keeps `align_to_*` from collapsing
/// neighbours into the same coordinate.
pub(crate) const EPSILON: f64 = 0.001;

pub struct Placer<'a> {
    vg: &'a mut VisualGraph,
}

impl<'a> Placer<'a> {
    pub fn new(vg: &'a mut VisualGraph) -> Self {
        Self { vg }
    }

    pub fn run(&mut self) {
        // Left-to-right is implemented by transposing, running top-to-bottom,
        // then transposing back.
        let need_transpose = !self.vg.orientation().is_top_to_bottom();
        if need_transpose {
            self.vg.transpose();
        }

        simple::do_it(self.vg);
        verify::do_it(self.vg);

        bk::BK::new(self.vg).do_it();

        verify::do_it(self.vg);
        port_align::do_it(self.vg);
        verify::do_it(self.vg);
        edge_fix::do_it(self.vg);
        compact::do_it(self.vg);
        // Tighten runs in the internal TB working frame (x = row axis,
        // y = rank axis). The post-transpose step below also flips
        // cluster bbox coords so codegen sees them in the user's
        // original orientation.
        tighten::do_it(self.vg);

        if need_transpose {
            self.vg.transpose();
            // Mirror node-position transpose on cluster bboxes so they
            // stay in the same frame as entity positions.
            for c in &mut self.vg.hierarchy.clusters {
                std::mem::swap(&mut c.x_min, &mut c.y_min);
                std::mem::swap(&mut c.x_max, &mut c.y_max);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rank-reduction and edge-cross optimization passes. They operate on the DAG
// only (no positions), so they live here rather than under a Visible-aware
// pass module.

/// Sink nodes greedily to shorten long edges, without growing the live-edge
/// count at any rank.
pub struct RankOptimizer<'a> {
    dag: &'a mut DAG,
}

impl<'a> RankOptimizer<'a> {
    pub fn new(dag: &'a mut DAG) -> Self {
        Self { dag }
    }

    fn try_to_sink(&mut self, node: NodeHandle) -> bool {
        let backs = self.dag.predecessors(node);
        let fwds = self.dag.successors(node);
        if backs.len() > fwds.len() || backs.len() + fwds.len() == 0 {
            return false;
        }
        let curr_rank = self.dag.level(node);
        let nearest_succ_level = fwds
            .iter()
            .map(|n| self.dag.level(*n))
            .min()
            .unwrap_or(self.dag.len());
        if nearest_succ_level > curr_rank + 1 {
            self.dag
                .update_node_rank_level(node, nearest_succ_level - 1, None);
            return true;
        }
        false
    }

    pub fn optimize(&mut self) {
        self.dag.verify();
        loop {
            let nodes: Vec<_> = self.dag.iter().collect();
            let any = nodes.into_iter().any(|n| self.try_to_sink(n));
            if !any {
                break;
            }
        }
    }
}

/// Reduce edge crossings by repeatedly swapping adjacent nodes within a
/// rank when the swap reduces total crossings. Iterates a fixed number of
/// times with periodic perturbation; keeps the best ordering seen.
pub struct EdgeCrossOptimizer<'a> {
    dag: &'a mut DAG,
    /// When set, refuse swaps that would cross a cluster boundary so
    /// each cluster's members stay contiguous in row order.
    hierarchy: Option<&'a HierarchyMap>,
}

impl<'a> EdgeCrossOptimizer<'a> {
    pub fn new(dag: &'a mut DAG) -> Self {
        Self { dag, hierarchy: None }
    }

    pub fn with_hierarchy(mut self, h: &'a HierarchyMap) -> Self {
        if !h.is_empty() {
            self.hierarchy = Some(h);
        }
        self
    }

    fn num_crossing(&self, a: NodeHandle, b: NodeHandle, row: &[NodeHandle]) -> usize {
        let a_succ = self.dag.successors(a);
        let a_pred = self.dag.predecessors(a);
        let b_succ = self.dag.successors(b);
        let b_pred = self.dag.predecessors(b);
        let mut sum = 0;
        let mut num_b = 0;
        for node in row {
            let on_a = a_succ.contains(node) || a_pred.contains(node);
            let on_b = b_succ.contains(node) || b_pred.contains(node);
            if on_a {
                sum += num_b;
            }
            if on_b {
                num_b += 1;
            }
        }
        sum
    }

    fn count_crossing_in_rows(&self, first: &[NodeHandle], second: &[NodeHandle]) -> usize {
        let mut sum = 0;
        for i in 0..first.len() {
            for j in i + 1..first.len() {
                sum += self.num_crossing(first[i], first[j], second);
            }
        }
        sum
    }

    fn count_crossed_edges(&self) -> usize {
        let levels = self.dag.num_levels();
        if levels < 2 {
            return 0;
        }
        (0..levels - 1)
            .map(|i| self.count_crossing_in_rows(self.dag.row(i), self.dag.row(i + 1)))
            .sum()
    }

    fn swap_crossed_edges_on_row(
        &mut self,
        row_idx: usize,
        scan_up: bool,
        scan_down: bool,
    ) -> bool {
        let num_rows = self.dag.num_levels();
        let prev_row = if row_idx > 0 && scan_up {
            self.dag.row(row_idx - 1).clone()
        } else {
            Vec::new()
        };
        let next_row = if row_idx + 1 < num_rows && scan_down {
            self.dag.row(row_idx + 1).clone()
        } else {
            Vec::new()
        };
        let mut row = self.dag.row(row_idx).clone();
        if row.len() < 2 {
            return false;
        }
        let mut changed = false;
        for i in 0..row.len() - 1 {
            let a = row[i];
            let b = row[i + 1];
            // Cluster gate: each cluster's row span stays contiguous, so
            // strangers and other clusters' members never get swapped
            // between two members of the same cluster. Sibling-cluster
            // relative order is fixed by `HierarchyMap::group_rows`
            // before mincross starts.
            if let Some(h) = self.hierarchy {
                if !h.same_family(a, b) {
                    continue;
                }
            }
            let ab = self.num_crossing(a, b, &prev_row) + self.num_crossing(a, b, &next_row);
            let ba = self.num_crossing(b, a, &prev_row) + self.num_crossing(b, a, &next_row);
            if ab > ba {
                row.swap(i, i + 1);
                changed = true;
            }
        }
        if changed {
            *self.dag.row_mut(row_idx) = row;
        }
        changed
    }

    fn swap_crossed_edges(&mut self, scan_up: bool, scan_down: bool) {
        loop {
            let mut changed = false;
            if scan_down {
                for i in 0..self.dag.num_levels() {
                    changed |= self.swap_crossed_edges_on_row(i, scan_up, scan_down);
                }
            }
            if scan_up {
                for i in (0..self.dag.num_levels()).rev() {
                    changed |= self.swap_crossed_edges_on_row(i, scan_up, scan_down);
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn rotate_rank(&mut self) {
        for i in 0..self.dag.num_levels() {
            self.dag.row_mut(i).rotate_left(1);
        }
    }

    fn perturb_rank(&mut self) {
        for i in 0..self.dag.num_levels() {
            let row = self.dag.row_mut(i);
            let len = row.len();
            for j in 0..len {
                row.swap((j * 17) % len, j);
            }
        }
    }

    pub fn optimize(&mut self) {
        self.dag.verify();
        let mut best_rank = self.dag.ranks().clone();
        let mut best_cnt = self.count_crossed_edges();
        for i in 0..50 {
            // Cycle through scan directions: both, up, down, down — same
            // pattern as the upstream impl; no special meaning beyond mixing
            // the search.
            let (up, down) = match i % 4 {
                0 => (true, true),
                1 => (true, false),
                2 | 3 => (false, true),
                _ => unreachable!(),
            };
            self.swap_crossed_edges(up, down);
            let new_cnt = self.count_crossed_edges();
            if new_cnt < best_cnt {
                best_rank = self.dag.ranks().clone();
                best_cnt = new_cnt;
            }
            // Rotation / perturbation shuffles each row wholesale, which
            // would break the cluster contiguity the same-family gate is
            // protecting. In hierarchy mode we rely on `group_rows`
            // having set a sane initial order and just let the gated
            // swap pass refine within each cluster.
            if self.hierarchy.is_none() {
                self.rotate_rank();
                if i % 10 == 0 {
                    self.perturb_rank();
                }
            }
        }
        *self.dag.ranks_mut() = best_rank;
    }
}
