//! Cluster / sub-graph annotation for hierarchical Sugiyama.
//!
//! `HierarchyMap` is a side-band carried alongside `VisualGraph`; the
//! per-pass machinery (rank assignment, mincross, BK) consults it to keep
//! a cluster's members contiguous in rank order and inside a shared
//! x-extent. Empty / absent map = no clusters = original flat behaviour.
//!
//! Owned and mutated by the layout pipeline. Built by codegen
//! (`codegen::cuca`) from `CucaDiagram.containers` and handed to
//! `VisualGraph::set_hierarchy`.
//!
//! Algorithms reference: Sander 1996, "Layout of Compound Directed Graphs".

use crate::layout::dag::{NodeHandle, DAG};

/// Index into a `HierarchyMap`'s `clusters` vec. Stable across the
/// layout pipeline.
pub type ClusterId = usize;

#[derive(Clone, Debug)]
pub struct HCluster {
    /// Outer cluster's id, or `None` for top-level.
    pub parent: Option<ClusterId>,
    /// Direct entity-children (handles into the VisualGraph DAG). Long
    /// edges' dummy nodes inherit from their source via `node_cluster`,
    /// so they're not listed here.
    pub direct_nodes: Vec<NodeHandle>,
    /// Direct child clusters.
    pub direct_children: Vec<ClusterId>,
    /// Rank span covered by all descendant nodes. Filled by
    /// `recompute_rank_span` after every rank-changing pass.
    pub rank_min: usize,
    pub rank_max: usize,
    /// x extent (in absolute coords) covered by all descendants after
    /// tighten. `f64::INFINITY` until `tighten` runs.
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    /// Padding around content (matches the painter's container PAD).
    pub pad: f64,
    /// Label band height (measure protocol) + buffer.
    pub label_band: f64,
    /// Minimum outer width imposed by the label.
    pub label_min_w: f64,
}

impl HCluster {
    pub fn new(parent: Option<ClusterId>) -> Self {
        Self {
            parent,
            direct_nodes: Vec::new(),
            direct_children: Vec::new(),
            rank_min: usize::MAX,
            rank_max: 0,
            x_min: f64::INFINITY,
            x_max: f64::NEG_INFINITY,
            y_min: f64::INFINITY,
            y_max: f64::NEG_INFINITY,
            pad: 0.0,
            label_band: 0.0,
            label_min_w: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HierarchyMap {
    /// `node_cluster[h.get_index()]` = innermost cluster directly owning
    /// `h`, or `None` for "no cluster". Resized lazily — see
    /// `cluster_of`.
    node_cluster: Vec<Option<ClusterId>>,
    pub clusters: Vec<HCluster>,
}

impl HierarchyMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    pub fn add_cluster(&mut self, parent: Option<ClusterId>) -> ClusterId {
        let id = self.clusters.len();
        self.clusters.push(HCluster::new(parent));
        if let Some(p) = parent {
            self.clusters[p].direct_children.push(id);
        }
        id
    }

    /// Register `h` as a direct member of `cluster`. Replaces any prior
    /// assignment; the caller is expected to call this once per real
    /// node.
    pub fn assign_node(&mut self, h: NodeHandle, cluster: ClusterId) {
        let idx = h.get_index();
        if self.node_cluster.len() <= idx {
            self.node_cluster.resize(idx + 1, None);
        }
        self.node_cluster[idx] = Some(cluster);
        self.clusters[cluster].direct_nodes.push(h);
    }

    /// Mirror `src`'s cluster onto `dst`. Used when the lowering pass
    /// inserts connector dummies along a long edge — they should inherit
    /// their source's cluster so cluster_bubble doesn't see a "stranger"
    /// node sitting inside the cluster's rank span.
    pub fn inherit_node(&mut self, src: NodeHandle, dst: NodeHandle) {
        let parent = self.cluster_of(src);
        if let Some(p) = parent {
            let idx = dst.get_index();
            if self.node_cluster.len() <= idx {
                self.node_cluster.resize(idx + 1, None);
            }
            self.node_cluster[idx] = Some(p);
            // NB: connector dummies are NOT listed in `direct_nodes` —
            // we only want real entities in that list for tighten's
            // bbox union. The cluster-of lookup still returns Some(p).
        }
    }

    pub fn cluster_of(&self, h: NodeHandle) -> Option<ClusterId> {
        self.node_cluster.get(h.get_index()).copied().flatten()
    }

    /// Iterate `c` and all of its ancestors, innermost first.
    pub fn ancestors(&self, c: ClusterId) -> Ancestors<'_> {
        Ancestors {
            map: self,
            cur: Some(c),
        }
    }

    /// Returns true iff `c` is an ancestor of (or equal to) the cluster
    /// containing `h`. Used by the edge-routing obstacle filter and by
    /// mincross's "same family" gate.
    pub fn is_inside(&self, h: NodeHandle, c: ClusterId) -> bool {
        match self.cluster_of(h) {
            None => false,
            Some(start) => self.ancestors(start).any(|a| a == c),
        }
    }

    /// Innermost common ancestor cluster of `a` and `b`. `None` when one
    /// side has no cluster or the two sides have no common ancestor.
    pub fn common_ancestor(&self, a: NodeHandle, b: NodeHandle) -> Option<ClusterId> {
        let ca = self.cluster_of(a)?;
        let cb = self.cluster_of(b)?;
        let chain_a: Vec<_> = self.ancestors(ca).collect();
        for x in self.ancestors(cb) {
            if chain_a.contains(&x) {
                return Some(x);
            }
        }
        None
    }

    /// Top-level ancestor of `c` (highest cluster with `parent: None`
    /// on `c`'s chain). Returns `c` itself when `c` is already
    /// top-level.
    pub fn top_ancestor(&self, c: ClusterId) -> ClusterId {
        let mut cur = c;
        while let Some(p) = self.clusters[cur].parent {
            cur = p;
        }
        cur
    }

    /// Sort key for a node's row position: outermost cluster id first,
    /// then nested cluster ids inside-out, then `usize::MAX` for the
    /// "no cluster" sentinel so strangers cluster at the right edge of
    /// the row.
    fn cluster_chain_key(&self, h: NodeHandle) -> Vec<usize> {
        let Some(direct) = self.cluster_of(h) else {
            return vec![usize::MAX];
        };
        let mut chain: Vec<_> = self.ancestors(direct).collect();
        chain.reverse();
        chain
    }

    /// Reorder every DAG row so cluster members are contiguous along
    /// the row, with the outermost cluster providing the major sort
    /// key. Stable: ties preserve the existing row order, so anything
    /// `RankOptimizer` learned about preferred positions is kept.
    /// Mincross's same-cluster gate then refines within each group
    /// without breaking it apart.
    pub fn group_rows(&self, dag: &mut DAG) {
        for r in 0..dag.num_levels() {
            let row = dag.row(r).clone();
            if row.len() < 2 {
                continue;
            }
            let mut indexed: Vec<(usize, NodeHandle, Vec<usize>)> = row
                .into_iter()
                .enumerate()
                .map(|(i, h)| (i, h, self.cluster_chain_key(h)))
                .collect();
            indexed.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
            let new_row: Vec<NodeHandle> = indexed.into_iter().map(|(_, h, _)| h).collect();
            *dag.row_mut(r) = new_row;
        }
    }

    /// Returns true iff `a` and `b` share the same direct cluster
    /// assignment (both None counts as "same family", which lets
    /// strangers mincross against each other freely).
    pub fn same_family(&self, a: NodeHandle, b: NodeHandle) -> bool {
        self.cluster_of(a) == self.cluster_of(b)
    }

    /// Reorder sibling top-level cluster groups in each rank row by the
    /// barycenter of their adjacent-row connections. Within a group,
    /// intra-group order is preserved — the same-cluster mincross swap
    /// already settled that. Returns true if any row was reordered.
    ///
    /// Run after the mincross swap loop converges. Two sweeps:
    /// forward (each row ordered by successor-row positions), then
    /// backward (by predecessor-row positions). Groups with no
    /// adjacent connections keep their current position as fallback.
    ///
    /// v1 only reorders top-level cluster groups (the most visible
    /// win for 3+ sibling clusters at the diagram's outer level).
    /// Nested-sibling reordering inside a shared parent stays in
    /// declaration order until a fixture demands it.
    pub fn reorder_cluster_groups(&self, dag: &mut DAG) -> bool {
        let num_levels = dag.num_levels();
        if num_levels < 2 || self.is_empty() {
            return false;
        }
        let mut any_change = false;
        for forward in [true, false] {
            let order: Vec<usize> = if forward {
                (0..num_levels).collect()
            } else {
                (0..num_levels).rev().collect()
            };
            for r in order {
                if self.reorder_row_by_barycenter(dag, r, forward) {
                    any_change = true;
                }
            }
        }
        any_change
    }

    fn reorder_row_by_barycenter(&self, dag: &mut DAG, row_idx: usize, forward: bool) -> bool {
        let row = dag.row(row_idx).clone();
        if row.len() < 2 {
            return false;
        }
        // Split the row into contiguous spans sharing the same
        // top-level cluster (`group_rows` already grouped by ancestor
        // chain so this scan is correct without a sort).
        let key = |n: NodeHandle| -> Option<ClusterId> {
            self.cluster_of(n).map(|c| self.top_ancestor(c))
        };
        let mut groups: Vec<(Option<ClusterId>, Vec<NodeHandle>)> = Vec::new();
        for &n in &row {
            let k = key(n);
            if let Some(last) = groups.last_mut() {
                if last.0 == k {
                    last.1.push(n);
                    continue;
                }
            }
            groups.push((k, vec![n]));
        }
        if groups.len() < 2 {
            return false;
        }

        // Reference row for barycenter lookup.
        let ref_idx = if forward {
            if row_idx + 1 >= dag.num_levels() {
                return false;
            }
            row_idx + 1
        } else {
            if row_idx == 0 {
                return false;
            }
            row_idx - 1
        };
        let ref_row = dag.row(ref_idx);
        let mut ref_pos: std::collections::HashMap<NodeHandle, usize> =
            std::collections::HashMap::with_capacity(ref_row.len());
        for (i, &n) in ref_row.iter().enumerate() {
            ref_pos.insert(n, i);
        }

        // Per-group barycenter: mean position of every member's
        // forward-neighbour (forward) or backward-neighbour (!forward)
        // inside `ref_row`. `None` when no neighbour lands in `ref_row`.
        let mut barys: Vec<Option<f64>> = Vec::with_capacity(groups.len());
        for (_, members) in &groups {
            let mut sum = 0.0;
            let mut count = 0usize;
            for &n in members {
                let adj = if forward {
                    dag.successors(n)
                } else {
                    dag.predecessors(n)
                };
                for a in adj {
                    if let Some(&p) = ref_pos.get(a) {
                        sum += p as f64;
                        count += 1;
                    }
                }
            }
            barys.push(if count == 0 { None } else { Some(sum / count as f64) });
        }

        // Stable sort: groups with a barycenter sort by it; groups
        // without keep their original index as the sort key so they
        // don't migrate.
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&i, &j| {
            let bi = barys[i].unwrap_or(i as f64);
            let bj = barys[j].unwrap_or(j as f64);
            bi.partial_cmp(&bj).unwrap_or(std::cmp::Ordering::Equal)
        });

        // No-op when sort didn't change anything.
        if order.iter().enumerate().all(|(i, &o)| i == o) {
            return false;
        }
        let new_row: Vec<NodeHandle> = order
            .into_iter()
            .flat_map(|i| groups[i].1.clone())
            .collect();
        *dag.row_mut(row_idx) = new_row;
        true
    }

    /// Recompute `rank_min` / `rank_max` for every cluster from the
    /// current DAG ranks. Called after any pass that reorders / re-ranks
    /// nodes.
    pub fn recompute_rank_span(&mut self, dag: &DAG) {
        for c in &mut self.clusters {
            c.rank_min = usize::MAX;
            c.rank_max = 0;
        }
        // Walk every node that has a cluster; propagate its rank up the
        // ancestor chain so a parent cluster's span covers all
        // descendants.
        for idx in 0..self.node_cluster.len() {
            let Some(direct) = self.node_cluster[idx] else {
                continue;
            };
            let h = NodeHandle::new(idx);
            let level = dag.level(h);
            let mut cur = Some(direct);
            while let Some(c) = cur {
                let cluster = &mut self.clusters[c];
                if level < cluster.rank_min {
                    cluster.rank_min = level;
                }
                if level > cluster.rank_max {
                    cluster.rank_max = level;
                }
                cur = cluster.parent;
            }
        }
    }
}

pub struct Ancestors<'a> {
    map: &'a HierarchyMap,
    cur: Option<ClusterId>,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = ClusterId;
    fn next(&mut self) -> Option<ClusterId> {
        let c = self.cur?;
        self.cur = self.map.clusters[c].parent;
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_walks_to_root() {
        let mut m = HierarchyMap::new();
        let root = m.add_cluster(None);
        let mid = m.add_cluster(Some(root));
        let leaf = m.add_cluster(Some(mid));
        let chain: Vec<_> = m.ancestors(leaf).collect();
        assert_eq!(chain, vec![leaf, mid, root]);
    }

    #[test]
    fn assign_and_lookup_node_cluster() {
        let mut m = HierarchyMap::new();
        let c = m.add_cluster(None);
        let h = NodeHandle::new(3);
        m.assign_node(h, c);
        assert_eq!(m.cluster_of(h), Some(c));
        // Untracked handle reports None.
        assert_eq!(m.cluster_of(NodeHandle::new(99)), None);
    }

    #[test]
    fn is_inside_checks_ancestors() {
        let mut m = HierarchyMap::new();
        let outer = m.add_cluster(None);
        let inner = m.add_cluster(Some(outer));
        let h = NodeHandle::new(0);
        m.assign_node(h, inner);
        assert!(m.is_inside(h, inner));
        assert!(m.is_inside(h, outer));
        let stranger = m.add_cluster(None);
        assert!(!m.is_inside(h, stranger));
    }

    #[test]
    fn common_ancestor_finds_nearest_shared() {
        let mut m = HierarchyMap::new();
        let outer = m.add_cluster(None);
        let inner_a = m.add_cluster(Some(outer));
        let inner_b = m.add_cluster(Some(outer));
        let a = NodeHandle::new(0);
        let b = NodeHandle::new(1);
        m.assign_node(a, inner_a);
        m.assign_node(b, inner_b);
        assert_eq!(m.common_ancestor(a, b), Some(outer));
    }

    #[test]
    fn reorder_cluster_groups_sorts_top_level_by_barycenter() {
        // Three top-level clusters declared in order C0, C1, C2.
        // Row 0: one member each (n0, n1, n2). Row 1: one member each
        // (n3, n4, n5). Edge structure forces row 0's barycenter-
        // friendly order to be C2, C0, C1.
        //
        //   row 0: [C0:n0] [C1:n1] [C2:n2]
        //   row 1: [C2:n5] [C0:n3] [C1:n4]
        //   edges: n0→n3, n1→n4, n2→n5
        //
        // n0's successor (n3) sits at index 1 in row 1 → bary 1.
        // n1's (n4) at index 2 → bary 2.
        // n2's (n5) at index 0 → bary 0.
        // Reorder pulls row 0 to [n2, n0, n1] (sorted by bary 0, 1, 2).
        let mut m = HierarchyMap::new();
        let c0 = m.add_cluster(None);
        let c1 = m.add_cluster(None);
        let c2 = m.add_cluster(None);
        for i in 0..6 {
            let h = NodeHandle::new(i);
            let cluster = match i {
                0 | 3 => c0,
                1 | 4 => c1,
                2 | 5 => c2,
                _ => unreachable!(),
            };
            m.assign_node(h, cluster);
        }

        let mut dag = DAG::new();
        for _ in 0..6 {
            dag.new_node();
        }
        // Place row 0 = [0, 1, 2], row 1 = [5, 3, 4].
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(3));
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(4));
        dag.add_edge(NodeHandle::new(2), NodeHandle::new(5));
        dag.recompute_node_ranks();
        // Manually set the row-1 order to [5, 3, 4] to force a
        // non-trivial barycenter for row 0.
        *dag.row_mut(1) = vec![NodeHandle::new(5), NodeHandle::new(3), NodeHandle::new(4)];

        let changed = m.reorder_cluster_groups(&mut dag);
        assert!(changed);
        let new_row_0 = dag.row(0).clone();
        assert_eq!(
            new_row_0,
            vec![NodeHandle::new(2), NodeHandle::new(0), NodeHandle::new(1)],
            "row 0 should be reordered by barycenter to put C2 (n2) leftmost",
        );
    }
}
