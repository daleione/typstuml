//! Port of `org.eclipse.elk.alg.common.networksimplex` —
//! `NGraph`/`NNode`/`NEdge` and the `NetworkSimplex` solver
//! (Gansner et al. rank assignment: feasible tight tree, cut values,
//! pivot loop, then normalize + optional balancing).
//!
//! The Java object graph becomes one flat store: `nodes`/`edges`
//! vectors plus an ordered `active_nodes` list standing in for
//! `NGraph.nodes` (subtree removal takes nodes out of the list and
//! reattaches them afterwards, preserving order semantics).

/// Java `NNode` (fields only; behavior lives on [`NetworkSimplex`]).
#[derive(Debug, Clone, Default)]
pub struct NNode {
    /// Caller-visible scratch id (Java `NNode.id`).
    pub id: usize,
    /// What this node stands for — the layered LNode index, if any.
    pub origin: Option<usize>,
    pub layer: i32,
    pub(crate) internal_id: usize,
    pub(crate) tree_node: bool,
    pub(crate) outgoing_edges: Vec<usize>,
    pub(crate) incoming_edges: Vec<usize>,
    pub(crate) unknown_cutvalues: Vec<usize>,
}

/// Java `NEdge`.
#[derive(Debug, Clone)]
pub struct NEdge {
    pub origin: Option<usize>,
    pub source: usize,
    pub target: usize,
    pub weight: f64,
    pub delta: i32,
    pub(crate) internal_id: usize,
    pub(crate) tree_edge: bool,
}

/// Java `NGraph` (as much as the layerer uses).
#[derive(Debug, Clone, Default)]
pub struct NGraph {
    pub nodes: Vec<NNode>,
    pub edges: Vec<NEdge>,
    /// Mirrors `NGraph.nodes` list membership/order; subtree removal
    /// operates on this, never on the backing vectors.
    active_nodes: Vec<usize>,
}

impl NGraph {
    pub fn add_node(&mut self, origin: Option<usize>) -> usize {
        self.nodes.push(NNode { origin, ..NNode::default() });
        let idx = self.nodes.len() - 1;
        self.active_nodes.push(idx);
        idx
    }

    pub fn add_edge(&mut self, source: usize, target: usize, weight: f64, delta: i32) -> usize {
        let idx = self.edges.len();
        self.edges.push(NEdge {
            origin: None,
            source,
            target,
            weight,
            delta,
            internal_id: 0,
            tree_edge: false,
        });
        self.nodes[source].outgoing_edges.push(idx);
        self.nodes[target].incoming_edges.push(idx);
        idx
    }

    fn other(&self, edge: usize, node: usize) -> usize {
        let e = &self.edges[edge];
        if e.source == node { e.target } else { e.source }
    }

    fn connected_edges(&self, node: usize) -> Vec<usize> {
        // Java `getConnectedEdges()`: outgoing then incoming.
        let n = &self.nodes[node];
        n.outgoing_edges.iter().chain(n.incoming_edges.iter()).copied().collect()
    }
}

const REMOVE_SUBTREES_THRESH: usize = 40;
const FUZZY_ST_ZERO: f64 = -1e-10;

/// Java `NetworkSimplex` (builder collapsed into plain parameters).
pub struct NetworkSimplex<'a> {
    graph: &'a mut NGraph,
    balance: bool,
    iteration_limit: usize,
    previous_layering_node_counts: Option<Vec<i32>>,

    // scratch state (Java fields)
    edges: Vec<usize>,
    tree_edges: Vec<usize>,
    sources: Vec<usize>,
    edge_visited: Vec<bool>,
    post_order: i32,
    po_id: Vec<i32>,
    lowest_po_id: Vec<i32>,
    cutvalue: Vec<f64>,
    subtree_nodes_stack: Vec<(usize, usize)>,
}

impl<'a> NetworkSimplex<'a> {
    pub fn for_graph(graph: &'a mut NGraph) -> Self {
        Self {
            graph,
            balance: false,
            iteration_limit: usize::MAX,
            previous_layering_node_counts: None,
            edges: Vec::new(),
            tree_edges: Vec::new(),
            sources: Vec::new(),
            edge_visited: Vec::new(),
            post_order: 1,
            po_id: Vec::new(),
            lowest_po_id: Vec::new(),
            cutvalue: Vec::new(),
            subtree_nodes_stack: Vec::new(),
        }
    }

    pub fn with_balancing(mut self, balance: bool) -> Self {
        self.balance = balance;
        self
    }

    pub fn with_iteration_limit(mut self, limit: usize) -> Self {
        self.iteration_limit = limit;
        self
    }

    pub fn with_previous_layering(mut self, counts: Option<Vec<i32>>) -> Self {
        self.previous_layering_node_counts = counts;
        self
    }

    fn initialize(&mut self) {
        let num_nodes = self.graph.active_nodes.len();
        for &n in &self.graph.active_nodes {
            self.graph.nodes[n].tree_node = false;
        }
        self.po_id = vec![0; num_nodes];
        self.lowest_po_id = vec![0; num_nodes];
        self.sources.clear();
        let mut the_edges: Vec<usize> = Vec::new();
        for (index, &node) in self.graph.active_nodes.iter().enumerate() {
            self.graph.nodes[node].internal_id = index;
            if self.graph.nodes[node].incoming_edges.is_empty() {
                self.sources.push(node);
            }
            the_edges.extend(self.graph.nodes[node].outgoing_edges.iter().copied());
        }
        for (counter, &edge) in the_edges.iter().enumerate() {
            self.graph.edges[edge].internal_id = counter;
            self.graph.edges[edge].tree_edge = false;
        }
        let num_edges = the_edges.len();
        self.cutvalue = vec![0.0; num_edges];
        self.edge_visited = vec![false; num_edges];
        self.edges = the_edges;
        self.tree_edges = Vec::with_capacity(num_edges);
        self.post_order = 1;
    }

    /// Java `execute()`.
    pub fn execute(mut self) {
        if self.graph.active_nodes.is_empty() {
            return;
        }
        for &n in &self.graph.active_nodes.clone() {
            self.graph.nodes[n].layer = 0;
        }
        let remove_subtrees = self.graph.active_nodes.len() >= REMOVE_SUBTREES_THRESH;
        if remove_subtrees {
            self.remove_subtrees();
        }
        self.initialize();
        self.feasible_tree();
        let mut e = self.leave_edge();
        let mut iter = 0usize;
        while let Some(leave) = e {
            if iter >= self.iteration_limit {
                break;
            }
            let enter = self.enter_edge(leave).expect("network simplex: no entering edge");
            self.exchange(leave, enter);
            e = self.leave_edge();
            iter += 1;
        }
        if remove_subtrees {
            self.reattach_subtrees();
        }
        let filling = self.normalize();
        if self.balance {
            self.do_balance(filling);
        }
    }

    /// Java `removeSubtrees()`.
    fn remove_subtrees(&mut self) {
        let mut leafs: std::collections::VecDeque<usize> = Default::default();
        for &node in &self.graph.active_nodes {
            if self.graph.connected_edges(node).len() == 1 {
                leafs.push_back(node);
            }
        }
        while let Some(node) = leafs.pop_front() {
            let connected = self.graph.connected_edges(node);
            if connected.is_empty() {
                continue;
            }
            let edge = connected[0];
            let is_out_edge = !self.graph.nodes[node].outgoing_edges.is_empty();
            let other = self.graph.other(edge, node);
            if is_out_edge {
                self.graph.nodes[other].incoming_edges.retain(|&e| e != edge);
            } else {
                self.graph.nodes[other].outgoing_edges.retain(|&e| e != edge);
            }
            if self.graph.connected_edges(other).len() == 1 {
                leafs.push_back(other);
            }
            self.subtree_nodes_stack.push((node, edge));
            self.graph.active_nodes.retain(|&n| n != node);
        }
    }

    /// Java `reattachSubtrees()`.
    fn reattach_subtrees(&mut self) {
        while let Some((node, edge)) = self.subtree_nodes_stack.pop() {
            let placed = self.graph.other(edge, node);
            if self.graph.edges[edge].target == node {
                self.graph.nodes[placed].outgoing_edges.push(edge);
                self.graph.nodes[node].layer =
                    self.graph.nodes[placed].layer + self.graph.edges[edge].delta;
            } else {
                self.graph.nodes[placed].incoming_edges.push(edge);
                self.graph.nodes[node].layer =
                    self.graph.nodes[placed].layer - self.graph.edges[edge].delta;
            }
            self.graph.active_nodes.push(node);
        }
    }

    /// Java `feasibleTree()`.
    fn feasible_tree(&mut self) {
        self.layering_topological_numbering();
        if !self.edges.is_empty() {
            self.edge_visited.fill(false);
            let first = self.graph.active_nodes[0];
            while self.tight_tree_dfs(first) < self.graph.active_nodes.len() {
                let e = self.minimal_slack().expect("disconnected tight tree");
                let e_ref = &self.graph.edges[e];
                let mut slack = self.graph.nodes[e_ref.target].layer
                    - self.graph.nodes[e_ref.source].layer
                    - e_ref.delta;
                if self.graph.nodes[e_ref.target].tree_node {
                    slack = -slack;
                }
                for &node in &self.graph.active_nodes {
                    if self.graph.nodes[node].tree_node {
                        self.graph.nodes[node].layer += slack;
                    }
                }
                self.edge_visited.fill(false);
                // Java also resets treeNode flags implicitly? No — it
                // keeps them; tightTreeDFS re-marks from scratch each
                // round but old marks persist. Kept identical.
            }
            self.edge_visited.fill(false);
            self.post_order = 1;
            self.postorder_traversal(first);
            self.cutvalues();
        }
    }

    /// Java `layeringTopologicalNumbering(sources)`.
    fn layering_topological_numbering(&mut self) {
        let mut incident = vec![0i32; self.graph.active_nodes.len()];
        for &node in &self.graph.active_nodes {
            incident[self.graph.nodes[node].internal_id] +=
                self.graph.nodes[node].incoming_edges.len() as i32;
        }
        let mut roots: std::collections::VecDeque<usize> = self.sources.iter().copied().collect();
        while let Some(node) = roots.pop_front() {
            for edge in self.graph.nodes[node].outgoing_edges.clone() {
                let target = self.graph.edges[edge].target;
                let candidate = self.graph.nodes[node].layer + self.graph.edges[edge].delta;
                if candidate > self.graph.nodes[target].layer {
                    self.graph.nodes[target].layer = candidate;
                }
                let tid = self.graph.nodes[target].internal_id;
                incident[tid] -= 1;
                if incident[tid] == 0 {
                    roots.push_back(target);
                }
            }
        }
    }

    /// Java `minimalSpan(node)`.
    fn minimal_span(&self, node: usize) -> (i32, i32) {
        let mut min_span_out = i32::MAX;
        let mut min_span_in = i32::MAX;
        for edge in self.graph.connected_edges(node) {
            let e = &self.graph.edges[edge];
            let current_span = self.graph.nodes[e.target].layer - self.graph.nodes[e.source].layer;
            if e.target == node && current_span < min_span_in {
                min_span_in = current_span;
            } else if e.target != node && current_span < min_span_out {
                min_span_out = current_span;
            }
        }
        if min_span_in == i32::MAX {
            min_span_in = -1;
        }
        if min_span_out == i32::MAX {
            min_span_out = -1;
        }
        (min_span_in, min_span_out)
    }

    /// Java `tightTreeDFS(node)`.
    fn tight_tree_dfs(&mut self, node: usize) -> usize {
        let mut node_count = 1usize;
        self.graph.nodes[node].tree_node = true;
        for edge in self.graph.connected_edges(node) {
            let internal = self.graph.edges[edge].internal_id;
            if !self.edge_visited[internal] {
                self.edge_visited[internal] = true;
                let opposite = self.graph.other(edge, node);
                if self.graph.edges[edge].tree_edge {
                    node_count += self.tight_tree_dfs(opposite);
                } else {
                    let e = &self.graph.edges[edge];
                    let tight = e.delta
                        == self.graph.nodes[e.target].layer - self.graph.nodes[e.source].layer;
                    if !self.graph.nodes[opposite].tree_node && tight {
                        self.graph.edges[edge].tree_edge = true;
                        self.tree_edges.push(edge);
                        node_count += self.tight_tree_dfs(opposite);
                    }
                }
            }
        }
        node_count
    }

    /// Java `minimalSlack()`.
    fn minimal_slack(&self) -> Option<usize> {
        let mut min_slack = i32::MAX;
        let mut min_slack_edge = None;
        for &edge in &self.edges {
            let e = &self.graph.edges[edge];
            if self.graph.nodes[e.source].tree_node ^ self.graph.nodes[e.target].tree_node {
                let cur_slack =
                    self.graph.nodes[e.target].layer - self.graph.nodes[e.source].layer - e.delta;
                if cur_slack < min_slack {
                    min_slack = cur_slack;
                    min_slack_edge = Some(edge);
                }
            }
        }
        min_slack_edge
    }

    /// Java `postorderTraversal(node)`.
    fn postorder_traversal(&mut self, node: usize) -> i32 {
        let mut lowest = i32::MAX;
        for edge in self.graph.connected_edges(node) {
            let internal = self.graph.edges[edge].internal_id;
            if self.graph.edges[edge].tree_edge && !self.edge_visited[internal] {
                self.edge_visited[internal] = true;
                let other = self.graph.other(edge, node);
                lowest = lowest.min(self.postorder_traversal(other));
            }
        }
        let iid = self.graph.nodes[node].internal_id;
        self.po_id[iid] = self.post_order;
        self.lowest_po_id[iid] = lowest.min(self.post_order);
        self.post_order += 1;
        self.lowest_po_id[iid]
    }

    /// Java `isInHead(node, edge)`.
    fn is_in_head(&self, node: usize, edge: usize) -> bool {
        let e = &self.graph.edges[edge];
        let (s, t, n) = (
            self.graph.nodes[e.source].internal_id,
            self.graph.nodes[e.target].internal_id,
            self.graph.nodes[node].internal_id,
        );
        if self.lowest_po_id[s] <= self.po_id[n]
            && self.po_id[n] <= self.po_id[s]
            && self.lowest_po_id[t] <= self.po_id[n]
            && self.po_id[n] <= self.po_id[t]
        {
            return self.po_id[s] >= self.po_id[t];
        }
        self.po_id[s] < self.po_id[t]
    }

    /// Java `cutvalues()`.
    fn cutvalues(&mut self) {
        let mut leafs: Vec<usize> = Vec::new();
        for &node in &self.graph.active_nodes {
            let mut tree_edge_count = 0;
            self.graph.nodes[node].unknown_cutvalues.clear();
            for edge in self.graph.connected_edges(node) {
                if self.graph.edges[edge].tree_edge {
                    self.graph.nodes[node].unknown_cutvalues.push(edge);
                    tree_edge_count += 1;
                }
            }
            if tree_edge_count == 1 {
                leafs.push(node);
            }
        }
        for &leaf in &leafs {
            let mut node = leaf;
            while self.graph.nodes[node].unknown_cutvalues.len() == 1 {
                let to_determine = self.graph.nodes[node].unknown_cutvalues[0];
                let to_id = self.graph.edges[to_determine].internal_id;
                self.cutvalue[to_id] = self.graph.edges[to_determine].weight;
                let source = self.graph.edges[to_determine].source;
                let target = self.graph.edges[to_determine].target;
                for edge in self.graph.connected_edges(node) {
                    if edge == to_determine {
                        continue;
                    }
                    let e_weight = self.graph.edges[edge].weight;
                    if self.graph.edges[edge].tree_edge {
                        let e_id = self.graph.edges[edge].internal_id;
                        if source == self.graph.edges[edge].source
                            || target == self.graph.edges[edge].target
                        {
                            self.cutvalue[to_id] -= self.cutvalue[e_id] - e_weight;
                        } else {
                            self.cutvalue[to_id] += self.cutvalue[e_id] - e_weight;
                        }
                    } else if node == source {
                        if self.graph.edges[edge].source == node {
                            self.cutvalue[to_id] += e_weight;
                        } else {
                            self.cutvalue[to_id] -= e_weight;
                        }
                    } else if self.graph.edges[edge].source == node {
                        self.cutvalue[to_id] -= e_weight;
                    } else {
                        self.cutvalue[to_id] += e_weight;
                    }
                }
                self.graph.nodes[source].unknown_cutvalues.retain(|&e| e != to_determine);
                self.graph.nodes[target].unknown_cutvalues.retain(|&e| e != to_determine);
                if source == node {
                    node = target;
                } else {
                    node = source;
                }
            }
        }
    }

    /// Java `leaveEdge()` — first tree edge (insertion order) with a
    /// negative cut value.
    fn leave_edge(&self) -> Option<usize> {
        self.tree_edges
            .iter()
            .copied()
            .find(|&edge| {
                self.graph.edges[edge].tree_edge
                    && self.cutvalue[self.graph.edges[edge].internal_id] < FUZZY_ST_ZERO
            })
    }

    /// Java `enterEdge(leave)`.
    fn enter_edge(&self, leave: usize) -> Option<usize> {
        let mut replace = None;
        let mut rep_slack = i32::MAX;
        for &edge in &self.edges {
            let e = &self.graph.edges[edge];
            if self.is_in_head(e.source, leave) && !self.is_in_head(e.target, leave) {
                let slack =
                    self.graph.nodes[e.target].layer - self.graph.nodes[e.source].layer - e.delta;
                if slack < rep_slack {
                    rep_slack = slack;
                    replace = Some(edge);
                }
            }
        }
        replace
    }

    /// Java `exchange(leave, enter)`.
    fn exchange(&mut self, leave: usize, enter: usize) {
        self.graph.edges[leave].tree_edge = false;
        self.tree_edges.retain(|&e| e != leave);
        self.graph.edges[enter].tree_edge = true;
        self.tree_edges.push(enter);

        let e = &self.graph.edges[enter];
        let mut delta =
            self.graph.nodes[e.target].layer - self.graph.nodes[e.source].layer - e.delta;
        if !self.is_in_head(e.target, leave) {
            delta = -delta;
        }
        let head_check: Vec<(usize, bool)> = self
            .graph
            .active_nodes
            .iter()
            .map(|&n| (n, self.is_in_head(n, leave)))
            .collect();
        for (node, in_head) in head_check {
            if !in_head {
                self.graph.nodes[node].layer += delta;
            }
        }
        self.post_order = 1;
        self.edge_visited.fill(false);
        let first = self.graph.active_nodes[0];
        self.postorder_traversal(first);
        self.cutvalues();
    }

    /// Java `normalize()`.
    fn normalize(&mut self) -> Vec<i32> {
        let mut highest = i32::MIN;
        let mut lowest = i32::MAX;
        for &node in &self.graph.active_nodes {
            lowest = lowest.min(self.graph.nodes[node].layer);
            highest = highest.max(self.graph.nodes[node].layer);
        }
        let mut filling = vec![0i32; (highest - lowest + 1) as usize];
        for &node in &self.graph.active_nodes.clone() {
            self.graph.nodes[node].layer -= lowest;
            filling[self.graph.nodes[node].layer as usize] += 1;
        }
        if let Some(previous) = &self.previous_layering_node_counts {
            for (layer_id, &node_cnt) in previous.iter().enumerate() {
                if layer_id >= filling.len() {
                    break;
                }
                filling[layer_id] += node_cnt;
            }
        }
        filling
    }

    /// Java `balance(filling)`.
    fn do_balance(&mut self, mut filling: Vec<i32>) {
        for &node in &self.graph.active_nodes.clone() {
            let n = &self.graph.nodes[node];
            if n.incoming_edges.len() == n.outgoing_edges.len() {
                let mut new_layer = n.layer;
                let (span_in, span_out) = self.minimal_span(node);
                let layer = self.graph.nodes[node].layer;
                let mut i = layer - span_in + 1;
                while i < layer + span_out {
                    if i >= 0
                        && (i as usize) < filling.len()
                        && filling[i as usize] < filling[new_layer as usize]
                    {
                        new_layer = i;
                    }
                    i += 1;
                }
                if filling[new_layer as usize] < filling[layer as usize] {
                    filling[layer as usize] -= 1;
                    filling[new_layer as usize] += 1;
                    self.graph.nodes[node].layer = new_layer;
                }
            }
        }
    }
}
