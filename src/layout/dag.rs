//! Ranked DAG. Stores node successors/predecessors plus a per-node "rank"
//! (row in the eventual layout). Rank is mutable — the optimizer passes
//! reorder rows and re-rank nodes to minimize edge crossings.

use std::cmp;

#[derive(Debug)]
pub struct DAG {
    nodes: Vec<Node>,
    ranks: RankType,
    levels: Vec<usize>,
}

#[derive(Copy, Clone, Default, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
pub struct NodeHandle {
    idx: usize,
}

impl NodeHandle {
    pub fn new(x: usize) -> Self {
        NodeHandle { idx: x }
    }
    pub fn get_index(&self) -> usize {
        self.idx
    }
}

impl From<usize> for NodeHandle {
    fn from(idx: usize) -> Self {
        NodeHandle { idx }
    }
}

#[derive(Debug)]
struct Node {
    successors: Vec<NodeHandle>,
    predecessors: Vec<NodeHandle>,
}

impl Node {
    fn new() -> Self {
        Node {
            successors: Vec::new(),
            predecessors: Vec::new(),
        }
    }
}

pub type RankType = Vec<Vec<NodeHandle>>;

#[derive(Debug)]
pub struct NodeIterator {
    curr: usize,
    last: usize,
}

impl Iterator for NodeIterator {
    type Item = NodeHandle;
    fn next(&mut self) -> Option<Self::Item> {
        if self.curr == self.last {
            return None;
        }
        let item = NodeHandle::from(self.curr);
        self.curr += 1;
        Some(item)
    }
}

impl Default for DAG {
    fn default() -> Self {
        Self::new()
    }
}

impl DAG {
    pub fn new() -> Self {
        DAG {
            nodes: Vec::new(),
            ranks: Vec::new(),
            levels: Vec::new(),
        }
    }

    pub fn iter(&self) -> NodeIterator {
        NodeIterator {
            curr: 0,
            last: self.nodes.len(),
        }
    }

    pub fn add_edge(&mut self, from: NodeHandle, to: NodeHandle) {
        self.nodes[from.idx].successors.push(to);
        self.nodes[to.idx].predecessors.push(from);
    }

    /// Returns true if there was an edge to remove.
    pub fn remove_edge(&mut self, from: NodeHandle, to: NodeHandle) -> bool {
        let succ = &mut self.nodes[from.idx].successors;
        let removed_succ = succ
            .iter()
            .position(|x| *x == to)
            .map(|p| succ.remove(p))
            .is_some();

        let pred = &mut self.nodes[to.idx].predecessors;
        let removed_pred = pred
            .iter()
            .position(|x| *x == from)
            .map(|p| pred.remove(p))
            .is_some();

        debug_assert_eq!(removed_pred, removed_succ);
        removed_pred
    }

    pub fn new_node(&mut self) -> NodeHandle {
        self.nodes.push(Node::new());
        self.levels.push(0);
        let node = NodeHandle::new(self.nodes.len() - 1);
        self.add_element_to_rank(node, 0);
        node
    }

    pub fn successors(&self, from: NodeHandle) -> &Vec<NodeHandle> {
        &self.nodes[from.idx].successors
    }

    pub fn predecessors(&self, from: NodeHandle) -> &Vec<NodeHandle> {
        &self.nodes[from.idx].predecessors
    }

    pub fn single_pred(&self, from: NodeHandle) -> Option<NodeHandle> {
        match self.nodes[from.idx].predecessors.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    pub fn single_succ(&self, from: NodeHandle) -> Option<NodeHandle> {
        match self.nodes[from.idx].successors.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    pub fn verify(&self) {
        for node in &self.nodes {
            for edge in &node.successors {
                assert!(edge.idx < self.nodes.len());
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            let from = NodeHandle::from(i);
            for dest in &node.successors {
                let cycle = self.is_reachable(*dest, from) && from != *dest;
                assert!(!cycle, "DAG contains a cycle");
            }
        }
        assert_eq!(self.count_nodes_in_ranks(), self.len());
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn is_reachable_inner(&self, from: NodeHandle, to: NodeHandle, visited: &mut [bool]) -> bool {
        if from == to {
            return true;
        }
        if visited[from.idx] {
            return false;
        }
        visited[from.idx] = true;
        for edge in &self.nodes[from.idx].successors {
            if self.is_reachable_inner(*edge, to, visited) {
                return true;
            }
        }
        false
    }

    pub fn is_reachable(&self, from: NodeHandle, to: NodeHandle) -> bool {
        if from == to {
            return true;
        }
        let mut visited = vec![false; self.nodes.len()];
        self.is_reachable_inner(from, to, &mut visited)
    }

    /// Reverse-postorder topological sort, iterative to keep stack flat.
    fn topological_sort(&self) -> Vec<NodeHandle> {
        let mut order: Vec<NodeHandle> = Vec::new();
        let mut visited = vec![false; self.nodes.len()];
        // Worklist entry: (node, is_post_visit). Pre-visits push the node
        // back as a post-visit and then push children for pre-visit.
        let mut worklist: Vec<(NodeHandle, bool)> = self.iter().map(|n| (n, false)).collect();

        while let Some((current, post)) = worklist.pop() {
            if post {
                order.push(current);
                continue;
            }
            if visited[current.idx] {
                continue;
            }
            visited[current.idx] = true;
            worklist.push((current, true));
            for edge in &self.nodes[current.idx].successors {
                worklist.push((*edge, false));
            }
        }
        order.reverse();
        order
    }

    pub fn num_levels(&self) -> usize {
        self.ranks.len()
    }

    pub fn row_mut(&mut self, level: usize) -> &mut Vec<NodeHandle> {
        &mut self.ranks[level]
    }

    pub fn row(&self, level: usize) -> &Vec<NodeHandle> {
        &self.ranks[level]
    }

    pub fn ranks(&self) -> &RankType {
        &self.ranks
    }

    pub fn ranks_mut(&mut self) -> &mut RankType {
        &mut self.ranks
    }

    fn add_element_to_rank(&mut self, elem: NodeHandle, level: usize) {
        while self.ranks.len() < level + 1 {
            self.ranks.push(Vec::new());
        }
        self.ranks[level].push(elem);
        self.levels[elem.get_index()] = level;
    }

    pub fn recompute_node_ranks(&mut self) {
        assert!(!self.is_empty(), "Sorting an empty graph");
        let order = self.topological_sort();
        let levels = self.compute_levels(&order);
        self.ranks.clear();
        for (i, level) in levels.iter().enumerate() {
            self.add_element_to_rank(NodeHandle::from(i), *level);
        }
    }

    fn count_nodes_in_ranks(&self) -> usize {
        self.ranks.iter().map(|r| r.len()).sum()
    }

    /// Move `node` to `new_level`; if `insert_before` is set, insert it
    /// just before that marker, otherwise append.
    pub fn update_node_rank_level(
        &mut self,
        node: NodeHandle,
        new_level: usize,
        insert_before: Option<NodeHandle>,
    ) {
        let curr_level = self.level(node);
        let row = &mut self.ranks[curr_level];
        let idx = row.iter().position(|x| *x == node).expect("node not found");
        row.remove(idx);

        while self.ranks.len() < new_level + 1 {
            self.ranks.push(Vec::new());
        }

        if let Some(marker) = insert_before {
            let row = &mut self.ranks[new_level];
            let pos = row
                .iter()
                .position(|x| *x == marker)
                .expect("marker not in target row");
            row.insert(pos, node);
        } else {
            self.ranks[new_level].push(node);
        }
        self.levels[node.get_index()] = new_level;
    }

    pub fn level(&self, node: NodeHandle) -> usize {
        self.levels[node.get_index()]
    }

    /// Levels via single-source longest-path: each successor is at least
    /// one above its predecessor's level.
    fn compute_levels(&self, order: &[NodeHandle]) -> Vec<usize> {
        let mut levels = vec![0usize; self.nodes.len()];
        for src in order {
            for dest in &self.nodes[src.idx].successors {
                if src.idx == dest.idx {
                    continue;
                }
                levels[dest.idx] = cmp::max(levels[dest.idx], levels[src.idx] + 1);
            }
        }
        levels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_chain_ranks() {
        let mut g = DAG::new();
        let h0 = g.new_node();
        let h1 = g.new_node();
        let h2 = g.new_node();
        g.add_edge(h0, h1);
        g.add_edge(h1, h2);
        g.recompute_node_ranks();
        g.verify();
        assert_eq!(g.level(h0), 0);
        assert_eq!(g.level(h1), 1);
        assert_eq!(g.level(h2), 2);
    }

    #[test]
    fn remove_edge_round_trip() {
        let mut g = DAG::new();
        let h0 = g.new_node();
        let h1 = g.new_node();
        g.add_edge(h0, h1);
        assert!(g.remove_edge(h0, h1));
        assert!(!g.remove_edge(h0, h1));
    }
}
