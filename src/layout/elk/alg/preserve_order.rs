//! Port of `SortByInputModelProcessor` and its two comparators
//! (`org.eclipse.elk.alg.layered.intermediate.preserveorder.*`),
//! the `SORT_BY_INPUT_ORDER_OF_MODEL` intermediate processor that runs
//! before phase 3 when `considerModelOrder != NONE`. It establishes the
//! model-order-respecting initial in-layer node (and port) order that
//! the layer sweep then tries to preserve.
//!
//! Scope notes for the flat/architecture case:
//! - `considerModelOrder = NODES_AND_EDGES`, so two real nodes (both
//!   with MODEL_ORDER) compare purely by model order; only dummies enter
//!   the previous-layer connection logic.
//! - group ordering is `ONLY_WITHIN_GROUP` (not `ENFORCED`), so
//!   `calculateModelOrderOrGroupModelOrder` is just the raw MODEL_ORDER
//!   (or -1 if absent).
//! - `portModelOrder` defaults false; `longEdgeStrategy = DUMMY_NODE_OVER`
//!   (`getModelOrderFromConnectedEdges` falls back to `i32::MAX`).
//! - a proper layering has no in-layer edges, so no LONG_EDGE dummy is a
//!   feedback dummy → `handleHelperDummyNodes` always returns 0.

use std::collections::{HashMap, HashSet};

use super::graph::{LGraphArena, LGraphId, LNodeId, LPortId, NodeType};
use super::options::{OrderingStrategy, PortSide};

/// Java `SortByInputModelProcessor.process` for one graph.
pub fn sort_by_input_model(arena: &mut LGraphArena, graph: LGraphId) {
    let strategy = arena.graphs[graph.0].props.consider_model_order;
    if strategy == OrderingStrategy::None {
        return;
    }
    let n_layers = arena.graphs[graph.0].layers.len();
    for layer_index in 0..n_layers {
        let prev_index = if layer_index == 0 { 0 } else { layer_index - 1 };
        let previous_layer = arena.graphs[graph.0].layers[prev_index].nodes.clone();

        // Sort nodes (beforePorts = true).
        let mut nodes = arena.graphs[graph.0].layers[layer_index].nodes.clone();
        let mut nc = ModelOrderNodeComparator::new(graph, previous_layer.clone(), strategy, true);
        insertion_sort_nodes(arena, &mut nodes, &mut nc);
        arena.graphs[graph.0].layers[layer_index].nodes = nodes.clone();

        // Sort each node's ports (skip order-fixed dummies).
        for &node in &nodes {
            let pc = arena.nodes[node.0].props.port_constraints;
            if !pc.is_order_fixed() {
                let target_map = long_edge_target_node_preprocessing(arena, node);
                let mut ports = arena.nodes[node.0].ports.clone();
                let mut pcmp =
                    ModelOrderPortComparator::new(graph, previous_layer.clone(), strategy, target_map);
                insertion_sort_ports(arena, &mut ports, &mut pcmp);
                arena.nodes[node.0].ports = ports;
            }
        }

        // Sort nodes again (beforePorts = false).
        let mut nodes = arena.graphs[graph.0].layers[layer_index].nodes.clone();
        let mut nc = ModelOrderNodeComparator::new(graph, previous_layer, strategy, false);
        insertion_sort_nodes(arena, &mut nodes, &mut nc);
        arena.graphs[graph.0].layers[layer_index].nodes = nodes;
    }
}

fn insertion_sort_nodes(
    arena: &LGraphArena,
    layer: &mut [LNodeId],
    cmp: &mut ModelOrderNodeComparator,
) {
    for i in 1..layer.len() {
        let temp = layer[i];
        let mut j = i;
        while j > 0 && cmp.compare(arena, layer[j - 1], temp) > 0 {
            layer[j] = layer[j - 1];
            j -= 1;
        }
        layer[j] = temp;
    }
    cmp.clear_transitive_ordering();
}

fn insertion_sort_ports(
    arena: &LGraphArena,
    ports: &mut [LPortId],
    cmp: &mut ModelOrderPortComparator,
) {
    for i in 1..ports.len() {
        let temp = ports[i];
        let mut j = i;
        while j > 0 && cmp.compare(arena, ports[j - 1], temp) > 0 {
            ports[j] = ports[j - 1];
            j -= 1;
        }
        ports[j] = temp;
    }
    cmp.clear_transitive_ordering();
}

/// group strategy is ONLY_WITHIN_GROUP (not ENFORCED) in scope, so this
/// is `MODEL_ORDER` or -1.
fn model_order_or(mo: Option<i32>) -> i32 {
    mo.unwrap_or(-1)
}

// ----------------------------------------------------------------------
// ModelOrderNodeComparator
// ----------------------------------------------------------------------

pub struct ModelOrderNodeComparator {
    #[allow(dead_code)]
    graph: LGraphId,
    previous_layer: Vec<LNodeId>,
    ordering_strategy: OrderingStrategy,
    bigger_than: HashMap<LNodeId, HashSet<LNodeId>>,
    smaller_than: HashMap<LNodeId, HashSet<LNodeId>>,
    #[allow(dead_code)]
    before_ports: bool,
}

impl ModelOrderNodeComparator {
    fn new(
        graph: LGraphId,
        previous_layer: Vec<LNodeId>,
        ordering_strategy: OrderingStrategy,
        before_ports: bool,
    ) -> Self {
        Self {
            graph,
            previous_layer,
            ordering_strategy,
            bigger_than: HashMap::new(),
            smaller_than: HashMap::new(),
            before_ports,
        }
    }

    fn clear_transitive_ordering(&mut self) {
        self.bigger_than.clear();
        self.smaller_than.clear();
    }

    fn has_model_order(arena: &LGraphArena, n: LNodeId) -> bool {
        arena.nodes[n.0].props.model_order.is_some()
    }

    // The contains_key/insert pairs below are ELK's exact
    // transitive-closure short-circuits (map lookup with an else-if early
    // return); `entry()` doesn't fit that shape, so allow it.
    #[allow(clippy::map_entry)]
    pub fn compare(&mut self, arena: &LGraphArena, n1: LNodeId, n2: LNodeId) -> i32 {
        // Transitive-closure short circuits.
        if !self.bigger_than.contains_key(&n1) {
            self.bigger_than.insert(n1, HashSet::new());
        } else if self.bigger_than[&n1].contains(&n2) {
            return 1;
        }
        if !self.bigger_than.contains_key(&n2) {
            self.bigger_than.insert(n2, HashSet::new());
        } else if self.bigger_than[&n2].contains(&n1) {
            return -1;
        }
        if !self.smaller_than.contains_key(&n1) {
            self.smaller_than.insert(n1, HashSet::new());
        } else if self.smaller_than[&n1].contains(&n2) {
            return -1;
        }
        if !self.smaller_than.contains_key(&n2) {
            self.smaller_than.insert(n2, HashSet::new());
        } else if self.bigger_than[&n2].contains(&n1) {
            return 1;
        }

        let n1_mo = Self::has_model_order(arena, n1);
        let n2_mo = Self::has_model_order(arena, n2);
        if self.ordering_strategy == OrderingStrategy::PreferEdges || !n1_mo || !n2_mo {
            let p1_src = self.first_source_port_from_previous_layer(arena, n1);
            let p2_src = self.first_source_port_from_previous_layer(arena, n2);

            if let (Some(p1s), Some(p2s)) = (p1_src, p2_src) {
                let p1_node = arena.ports[p1s.0].owner.unwrap();
                let p2_node = arena.ports[p2s.0].owner.unwrap();
                if p1_node == p2_node {
                    for &port in &arena.nodes[p1_node.0].ports {
                        if port == p1s {
                            self.update(n2, n1);
                            return -1;
                        } else if port == p2s {
                            self.update(n1, n2);
                            return 1;
                        }
                    }
                }
                // Order by position of the connected node in the previous layer.
                for &prev in &self.previous_layer.clone() {
                    if prev == p1_node {
                        self.update(n2, n1);
                        return -1;
                    } else if prev == p2_node {
                        self.update(n1, n2);
                        return 1;
                    }
                }
            }

            if p1_src.is_some() != p2_src.is_some() {
                // handleHelperDummyNodes is always 0 in scope (no feedback dummies).
                if !n1_mo || !n2_mo {
                    let n1e = self.model_order_from_connected_edges(arena, n1);
                    let n2e = self.model_order_from_connected_edges(arena, n2);
                    if n1e > n2e {
                        self.update(n1, n2);
                        return 1;
                    } else {
                        self.update(n2, n1);
                        return -1;
                    }
                }
            }
            // Both null, or fall-through: handled by model-order block below.
        }

        if n1_mo && n2_mo {
            let n1_order = model_order_or(arena.nodes[n1.0].props.model_order);
            let n2_order = model_order_or(arena.nodes[n2.0].props.model_order);
            if n1_order > n2_order {
                self.update(n1, n2);
                1
            } else {
                self.update(n2, n1);
                -1
            }
        } else {
            self.update(n2, n1);
            -1
        }
    }

    /// Java: first port whose first incoming edge originates in the
    /// immediately-previous layer; returns that edge's source port.
    fn first_source_port_from_previous_layer(
        &self,
        arena: &LGraphArena,
        n: LNodeId,
    ) -> Option<LPortId> {
        let node_layer = arena.nodes[n.0].layer.unwrap();
        for &p in &arena.nodes[n.0].ports {
            if let Some(&edge) = arena.ports[p.0].incoming_edges.first() {
                let src = arena.edges[edge.0].source.unwrap();
                let src_node = arena.ports[src.0].owner.unwrap();
                if node_layer > 0 && arena.nodes[src_node.0].layer == Some(node_layer - 1) {
                    return Some(src);
                }
            }
        }
        None
    }

    fn model_order_from_connected_edges(&self, arena: &LGraphArena, n: LNodeId) -> i32 {
        for &p in &arena.nodes[n.0].ports {
            if let Some(&edge) = arena.ports[p.0].incoming_edges.first() {
                // longEdgeStrategy = DUMMY_NODE_OVER only matters for the
                // no-edge fallback; here an incoming edge exists.
                return model_order_or(arena.edges[edge.0].props.model_order);
            }
        }
        // DUMMY_NODE_OVER.returnValue()
        i32::MAX
    }

    /// Java `updateBiggerAndSmallerAssociations(bigger, smaller)`.
    fn update(&mut self, bigger: LNodeId, smaller: LNodeId) {
        let smaller_bigger_than: Vec<LNodeId> =
            self.bigger_than.get(&smaller).into_iter().flatten().copied().collect();
        let bigger_smaller_than: Vec<LNodeId> =
            self.smaller_than.get(&bigger).into_iter().flatten().copied().collect();

        self.bigger_than.entry(bigger).or_default().insert(smaller);
        self.smaller_than.entry(smaller).or_default().insert(bigger);

        for very_small in &smaller_bigger_than {
            self.bigger_than.entry(bigger).or_default().insert(*very_small);
            let e = self.smaller_than.entry(*very_small).or_default();
            e.insert(bigger);
            for &x in &bigger_smaller_than {
                e.insert(x);
            }
        }
        for very_big in &bigger_smaller_than {
            self.smaller_than.entry(smaller).or_default().insert(*very_big);
            let e = self.bigger_than.entry(*very_big).or_default();
            e.insert(smaller);
            for &x in &smaller_bigger_than {
                e.insert(x);
            }
        }
    }
}

// ----------------------------------------------------------------------
// ModelOrderPortComparator
// ----------------------------------------------------------------------

pub struct ModelOrderPortComparator {
    #[allow(dead_code)]
    graph: LGraphId,
    previous_layer: Vec<LNodeId>,
    #[allow(dead_code)]
    strategy: OrderingStrategy,
    target_node_model_order: HashMap<LNodeId, i32>,
    bigger_than: HashMap<LPortId, HashSet<LPortId>>,
    smaller_than: HashMap<LPortId, HashSet<LPortId>>,
}

impl ModelOrderPortComparator {
    fn new(
        graph: LGraphId,
        previous_layer: Vec<LNodeId>,
        strategy: OrderingStrategy,
        target_node_model_order: HashMap<LNodeId, i32>,
    ) -> Self {
        Self {
            graph,
            previous_layer,
            strategy,
            target_node_model_order,
            bigger_than: HashMap::new(),
            smaller_than: HashMap::new(),
        }
    }

    fn clear_transitive_ordering(&mut self) {
        self.bigger_than.clear();
        self.smaller_than.clear();
    }

    /// The port's side in the normalized rightward frame (NORTH→WEST,
    /// SOUTH→EAST): ELK compares sides after its import rotation, which
    /// this port skips — group-node ports stay on NORTH/SOUTH here.
    fn side(arena: &LGraphArena, p: LPortId) -> PortSide {
        match arena.ports[p.0].side {
            PortSide::North => PortSide::West,
            PortSide::South => PortSide::East,
            s => s,
        }
    }
    fn has_incoming(arena: &LGraphArena, p: LPortId) -> bool {
        !arena.ports[p.0].incoming_edges.is_empty()
    }
    fn has_outgoing(arena: &LGraphArena, p: LPortId) -> bool {
        !arena.ports[p.0].outgoing_edges.is_empty()
    }

    #[allow(clippy::map_entry)]
    pub fn compare(&mut self, arena: &LGraphArena, p1: LPortId, p2: LPortId) -> i32 {
        if !self.bigger_than.contains_key(&p1) {
            self.bigger_than.insert(p1, HashSet::new());
        } else if self.bigger_than[&p1].contains(&p2) {
            return 1;
        }
        if !self.bigger_than.contains_key(&p2) {
            self.bigger_than.insert(p2, HashSet::new());
        } else if self.bigger_than[&p2].contains(&p1) {
            return -1;
        }
        if !self.smaller_than.contains_key(&p1) {
            self.smaller_than.insert(p1, HashSet::new());
        } else if self.smaller_than[&p1].contains(&p2) {
            return -1;
        }
        if !self.smaller_than.contains_key(&p2) {
            self.smaller_than.insert(p2, HashSet::new());
        } else if self.bigger_than[&p2].contains(&p1) {
            return 1;
        }

        let s1 = Self::side(arena, p1);
        let s2 = Self::side(arena, p2);
        if s1 != s2 {
            let result = (s1 as i32) - (s2 as i32);
            if result > 0 {
                self.update(p1, p2, 1);
            } else {
                self.update(p2, p1, 1);
            }
            return result;
        }

        let mut reverse_order = 1;

        // Incoming ports: order by the node they connect to in the previous layer.
        if Self::has_incoming(arena, p1) && Self::has_incoming(arena, p2) {
            if (s1 == PortSide::West && s2 == PortSide::West)
                || (s1 == PortSide::North && s2 == PortSide::North)
                || (s1 == PortSide::South && s2 == PortSide::South)
            {
                reverse_order = -reverse_order;
            }
            let p1_src = arena.edges[arena.ports[p1.0].incoming_edges[0].0].source.unwrap();
            let p2_src = arena.edges[arena.ports[p2.0].incoming_edges[0].0].source.unwrap();
            let p1_node = arena.ports[p1_src.0].owner.unwrap();
            let p2_node = arena.ports[p2_src.0].owner.unwrap();
            if p1_node == p2_node {
                for &port in &arena.nodes[p1_node.0].ports {
                    if port == p1_src {
                        self.update(p2, p1, reverse_order);
                        return -reverse_order;
                    } else if port == p2_src {
                        self.update(p1, p2, reverse_order);
                        return reverse_order;
                    }
                }
            }
            // (both-long-edge-in-same-layer branch is feedback-only → skipped in scope)
            let in_prev = self.check_reference_layer(p1_node, p2_node);
            if in_prev != 0 {
                if in_prev > 0 {
                    self.update(p1, p2, reverse_order);
                    return reverse_order;
                } else {
                    self.update(p2, p1, reverse_order);
                    return -reverse_order;
                }
            }
            // portModelOrder defaults false → no further branch here.
        }

        // Outgoing ports: order by their edges' model order (bundled by target node).
        if Self::has_outgoing(arena, p1) && Self::has_outgoing(arena, p2) {
            if (s1 == PortSide::West && s2 == PortSide::West)
                || (s1 == PortSide::South && s2 == PortSide::South)
            {
                reverse_order = -reverse_order;
            }
            let p1_target = arena.ports[p1.0].props.long_edge_target_node;
            let p2_target = arena.ports[p2.0].props.long_edge_target_node;

            // strategy PREFER_NODES branch omitted (NODES_AND_EDGES in scope).
            let mut p1_order = 0;
            let mut p2_order = 0;
            if arena.edges[arena.ports[p1.0].outgoing_edges[0].0].props.model_order.is_some() {
                p1_order =
                    model_order_or(arena.edges[arena.ports[p1.0].outgoing_edges[0].0].props.model_order);
            }
            if arena.edges[arena.ports[p2.0].outgoing_edges[0].0].props.model_order.is_some() {
                p2_order =
                    model_order_or(arena.edges[arena.ports[p2.0].outgoing_edges[0].0].props.model_order);
            }

            if p1_target.is_some() && p1_target == p2_target {
                if p1_order > p2_order {
                    self.update(p1, p2, reverse_order);
                    return reverse_order;
                } else {
                    self.update(p2, p1, reverse_order);
                    return -reverse_order;
                }
            }
            if let Some(t) = p1_target {
                if let Some(&mo) = self.target_node_model_order.get(&t) {
                    p1_order = mo;
                }
            }
            if let Some(t) = p2_target {
                if let Some(&mo) = self.target_node_model_order.get(&t) {
                    p2_order = mo;
                }
            }
            if p1_order > p2_order {
                self.update(p1, p2, reverse_order);
                return reverse_order;
            } else {
                self.update(p2, p1, reverse_order);
                return -reverse_order;
            }
        }

        // Outgoing before incoming; otherwise a stable fallback.
        if Self::has_incoming(arena, p1) && Self::has_outgoing(arena, p2) {
            self.update(p1, p2, reverse_order);
            1
        } else if Self::has_outgoing(arena, p1) && Self::has_incoming(arena, p2) {
            self.update(p2, p1, reverse_order);
            -1
        } else {
            self.update(p2, p1, reverse_order);
            -reverse_order
        }
    }

    fn check_reference_layer(&self, p1_node: LNodeId, p2_node: LNodeId) -> i32 {
        for &node in &self.previous_layer {
            if node == p1_node {
                return -1;
            } else if node == p2_node {
                return 1;
            }
        }
        0
    }

    fn update(&mut self, bigger_ori: LPortId, smaller_ori: LPortId, reverse_order: i32) {
        let (bigger, smaller) =
            if reverse_order < 0 { (smaller_ori, bigger_ori) } else { (bigger_ori, smaller_ori) };
        let smaller_bigger_than: Vec<LPortId> =
            self.bigger_than.get(&smaller).into_iter().flatten().copied().collect();
        let bigger_smaller_than: Vec<LPortId> =
            self.smaller_than.get(&bigger).into_iter().flatten().copied().collect();
        self.bigger_than.entry(bigger).or_default().insert(smaller);
        self.smaller_than.entry(smaller).or_default().insert(bigger);
        for very_small in &smaller_bigger_than {
            self.bigger_than.entry(bigger).or_default().insert(*very_small);
            let e = self.smaller_than.entry(*very_small).or_default();
            e.insert(bigger);
            for &x in &bigger_smaller_than {
                e.insert(x);
            }
        }
        for very_big in &bigger_smaller_than {
            self.smaller_than.entry(smaller).or_default().insert(*very_big);
            let e = self.bigger_than.entry(*very_big).or_default();
            e.insert(smaller);
            for &x in &smaller_bigger_than {
                e.insert(x);
            }
        }
    }
}

/// Java `SortByInputModelProcessor.longEdgeTargetNodePreprocessing`:
/// map each outgoing port's real (long-edge) target node to the minimal
/// model order of the edges reaching it, and memoize the target node on
/// the port.
fn long_edge_target_node_preprocessing(
    arena: &mut LGraphArena,
    node: LNodeId,
) -> HashMap<LNodeId, i32> {
    let mut target_node_model_order: HashMap<LNodeId, i32> = HashMap::new();
    for p in arena.nodes[node.0].ports.clone() {
        if arena.ports[p.0].outgoing_edges.is_empty() {
            continue;
        }
        let target_node = get_target_node(arena, p);
        arena.ports[p.0].props.long_edge_target_node = target_node;
        if let Some(tn) = target_node {
            let edge = arena.ports[p.0].outgoing_edges[0];
            if !arena.edges[edge.0].props.reversed {
                let mo = model_order_or(arena.edges[edge.0].props.model_order);
                let prev = target_node_model_order.get(&tn).copied().unwrap_or(i32::MAX);
                target_node_model_order.insert(tn, mo.min(prev));
            }
        }
    }
    target_node_model_order
}

/// Java `getTargetNode(port)`: follow the outgoing edge through the
/// long-edge dummy chain to the real target node.
fn get_target_node(arena: &LGraphArena, port: LPortId) -> Option<LNodeId> {
    let mut edge = arena.ports[port.0].outgoing_edges[0];
    loop {
        let node = arena.edge_target_node(edge).unwrap();
        if let Some(let_target) = arena.nodes[node.0].props.long_edge_target {
            return arena.ports[let_target.0].owner;
        }
        if arena.nodes[node.0].node_type != NodeType::Normal {
            if let Some(&next) = arena.node_outgoing_edges(node).first() {
                edge = next;
            } else {
                return None;
            }
        } else {
            return Some(node);
        }
    }
}
