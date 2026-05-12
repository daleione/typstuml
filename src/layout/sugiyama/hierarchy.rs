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

    /// Reorder sibling cluster groups in each rank row by the
    /// barycenter of their adjacent-row connections. Intra-group order
    /// is preserved.
    ///
    /// Depth-aware: at each cluster nesting depth (0 = top-level,
    /// 1 = direct children of top-level, …, up to the deepest
    /// cluster), do a forward + backward sweep. At depth `d` we
    /// look for contiguous row-spans whose ancestor chain agrees up
    /// to depth `d-1` ("siblings under the same depth-`d-1` parent")
    /// and reorder their depth-`d` sub-groups. This handles the
    /// `Outer > {InnerA, InnerB}` case as well as the original
    /// top-level case.
    pub fn reorder_cluster_groups(&self, dag: &mut DAG) -> bool {
        let num_levels = dag.num_levels();
        if num_levels < 2 || self.is_empty() {
            return false;
        }
        let max_depth = self.max_cluster_depth();
        let mut any_change = false;
        for depth in 0..=max_depth {
            for forward in [true, false] {
                let rows: Vec<usize> = if forward {
                    (0..num_levels).collect()
                } else {
                    (0..num_levels).rev().collect()
                };
                for r in rows {
                    if self.reorder_row_at_depth(dag, r, forward, depth) {
                        any_change = true;
                    }
                }
            }
        }
        any_change
    }

    /// Max depth across the cluster forest. Depth 0 = top-level
    /// (`parent: None`); a cluster's depth = number of ancestors
    /// strictly above it.
    fn max_cluster_depth(&self) -> usize {
        let mut max_d = 0;
        for c in 0..self.clusters.len() {
            // `ancestors(c)` yields c, c.parent, …, root. Count includes c
            // itself, so depth-of-c = count-1.
            let d = self.ancestors(c).count().saturating_sub(1);
            if d > max_d {
                max_d = d;
            }
        }
        max_d
    }

    /// Outermost-first ancestor chain of `n`'s direct cluster, or
    /// empty for unclustered nodes.
    fn outermost_chain(&self, n: NodeHandle) -> Vec<ClusterId> {
        let direct = match self.cluster_of(n) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut chain: Vec<_> = self.ancestors(direct).collect();
        chain.reverse();
        chain
    }

    /// Reorder one rank row at a specific cluster depth. Groups with
    /// matching `outermost_chain[..depth]` are treated as siblings
    /// under the same depth-`depth-1` parent; their depth-`depth`
    /// children get sorted by barycenter.
    fn reorder_row_at_depth(
        &self,
        dag: &mut DAG,
        row_idx: usize,
        forward: bool,
        depth: usize,
    ) -> bool {
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
        let row = dag.row(row_idx).clone();
        if row.len() < 2 {
            return false;
        }

        // Pre-compute per-row ancestor chains once; the row is walked
        // multiple times below.
        let chains: Vec<Vec<ClusterId>> = row.iter().map(|&n| self.outermost_chain(n)).collect();

        let ref_row = dag.row(ref_idx).clone();
        let mut ref_pos: std::collections::HashMap<NodeHandle, usize> =
            std::collections::HashMap::with_capacity(ref_row.len());
        for (i, &n) in ref_row.iter().enumerate() {
            ref_pos.insert(n, i);
        }

        let prefix_at = |idx: usize| -> &[ClusterId] {
            let c = &chains[idx];
            &c[..depth.min(c.len())]
        };
        let cluster_at = |idx: usize| -> Option<ClusterId> {
            chains[idx].get(depth).copied()
        };

        let mut new_row: Vec<NodeHandle> = Vec::with_capacity(row.len());
        let mut i = 0;
        let mut changed = false;
        while i < row.len() {
            // Span of contiguous nodes sharing the same parent prefix.
            let prefix_i = prefix_at(i).to_vec();
            let mut j = i + 1;
            while j < row.len() && prefix_at(j) == prefix_i.as_slice() {
                j += 1;
            }
            // Split this span by depth-`depth` cluster.
            let mut sub_spans: Vec<(Option<ClusterId>, Vec<NodeHandle>)> = Vec::new();
            for k in i..j {
                let key = cluster_at(k);
                if let Some(last) = sub_spans.last_mut() {
                    if last.0 == key {
                        last.1.push(row[k]);
                        continue;
                    }
                }
                sub_spans.push((key, vec![row[k]]));
            }
            if sub_spans.len() < 2 {
                new_row.extend(sub_spans.into_iter().flat_map(|(_, v)| v));
                i = j;
                continue;
            }

            // Per-sub-span barycenter against ref_row.
            let mut barys: Vec<Option<f64>> = Vec::with_capacity(sub_spans.len());
            for (_, members) in &sub_spans {
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

            let mut order: Vec<usize> = (0..sub_spans.len()).collect();
            order.sort_by(|&a, &b| {
                let ba = barys[a].unwrap_or(a as f64);
                let bb = barys[b].unwrap_or(b as f64);
                ba.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
            });
            if order.iter().enumerate().any(|(idx, &o)| idx != o) {
                changed = true;
            }
            for &k in &order {
                new_row.extend_from_slice(&sub_spans[k].1);
            }
            i = j;
        }

        if changed {
            *dag.row_mut(row_idx) = new_row;
        }
        changed
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

    #[test]
    fn reorder_cluster_groups_sorts_nested_siblings_inside_parent() {
        // OuterSrc contains InnerA{a0}, InnerB{b0}, InnerC{c0} at rank 0.
        // OuterDst contains SinkX{x0}, SinkY{y0}, SinkZ{z0} at rank 1.
        // Both outers wrap their children so depth-0 finds nothing to
        // reorder; the work belongs to depth-1's nested-sibling pass.
        // Edges a0→z0, b0→y0, c0→x0 force the inner clusters under
        // OuterSrc to reorder to [InnerC, InnerB, InnerA].
        let mut m = HierarchyMap::new();
        let outer_src = m.add_cluster(None);
        let inner_a = m.add_cluster(Some(outer_src));
        let inner_b = m.add_cluster(Some(outer_src));
        let inner_c = m.add_cluster(Some(outer_src));
        let outer_dst = m.add_cluster(None);
        let sink_x = m.add_cluster(Some(outer_dst));
        let sink_y = m.add_cluster(Some(outer_dst));
        let sink_z = m.add_cluster(Some(outer_dst));
        for i in 0..6 {
            let h = NodeHandle::new(i);
            let cluster = match i {
                0 => inner_a,
                1 => inner_b,
                2 => inner_c,
                3 => sink_x,
                4 => sink_y,
                5 => sink_z,
                _ => unreachable!(),
            };
            m.assign_node(h, cluster);
        }

        let mut dag = DAG::new();
        for _ in 0..6 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(5)); // a0 → z0
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(4)); // b0 → y0
        dag.add_edge(NodeHandle::new(2), NodeHandle::new(3)); // c0 → x0
        dag.recompute_node_ranks();
        // Force row 0 to declaration order [a0, b0, c0], row 1 to
        // [x0, y0, z0]; mincross gate keeps them stable.
        *dag.row_mut(0) = vec![NodeHandle::new(0), NodeHandle::new(1), NodeHandle::new(2)];
        *dag.row_mut(1) = vec![NodeHandle::new(3), NodeHandle::new(4), NodeHandle::new(5)];

        let changed = m.reorder_cluster_groups(&mut dag);
        assert!(changed);
        assert_eq!(
            dag.row(0).clone(),
            vec![NodeHandle::new(2), NodeHandle::new(1), NodeHandle::new(0)],
            "Outer's inner clusters should reorder to [InnerC, InnerB, InnerA] \
             so the cross-cluster edges run straight down",
        );
    }
}
