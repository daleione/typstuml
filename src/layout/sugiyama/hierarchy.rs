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
}
