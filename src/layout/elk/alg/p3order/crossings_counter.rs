//! Port of
//! `org.eclipse.elk.alg.layered.p3order.counting.CrossingsCounter`
//! (between-layer and in-layer paths).
//!
//! Scope: north/south-port crossings need `NORTH_SOUTH_PORT` dummies,
//! which don't occur in flat/architecture draw-uml inputs, so
//! `countNorthSouthPortCrossingsInLayer` and the switch-in-both-orders
//! helpers (greedy switch, added later) are omitted for now.
//!
//! The counter transfers between-layer crossings into the in-layer
//! counting problem by "folding" the right layer downward, then sweeps
//! port endpoints through a [`BinaryIndexedTree`]: as each port is
//! visited, `rank(endPosition)` yields how many already-placed
//! endpoints lie above the new edge — i.e. the crossings it introduces.

use super::super::graph::{LEdgeId, LGraphArena, LNodeId, LPortId};
use super::super::options::PortSide;
use super::binary_indexed_tree::BinaryIndexedTree;

pub struct CrossingsCounter<'a> {
    arena: &'a LGraphArena,
    /// Indexed by port arena id (`LPortId.0`); shared scratch array.
    port_positions: Vec<i32>,
    index_tree: BinaryIndexedTree,
    ends: Vec<usize>,
}

impl<'a> CrossingsCounter<'a> {
    /// `port_positions` must be sized to `arena.ports.len()` (ELK passes
    /// a reused array indexed by the graph-unique `port.id`).
    pub fn new(arena: &'a LGraphArena, port_positions: Vec<i32>) -> Self {
        Self { arena, port_positions, index_tree: BinaryIndexedTree::new(0), ends: Vec::new() }
    }

    /// Java `countCrossingsBetweenLayers(left, right)`.
    pub fn count_crossings_between_layers(
        &mut self,
        left_layer_nodes: &[LNodeId],
        right_layer_nodes: &[LNodeId],
    ) -> i32 {
        let ports = self.init_port_positions_counter_clockwise(left_layer_nodes, right_layer_nodes);
        self.index_tree = BinaryIndexedTree::new(ports.len());
        self.count_crossings_on_ports(&ports)
    }

    /// Java `countInLayerCrossingsOnSide(nodes, side)`.
    pub fn count_in_layer_crossings_on_side(
        &mut self,
        nodes: &[LNodeId],
        side: PortSide,
    ) -> i32 {
        let mut ports = Vec::new();
        self.init_positions(nodes, &mut ports, side, true);
        self.index_tree = BinaryIndexedTree::new(ports.len());
        self.count_in_layer_crossings_on_ports(&ports)
    }

    // -- position setup ------------------------------------------------

    fn init_port_positions_counter_clockwise(
        &mut self,
        left: &[LNodeId],
        right: &[LNodeId],
    ) -> Vec<LPortId> {
        let mut ports = Vec::new();
        self.init_positions(left, &mut ports, PortSide::East, true);
        self.init_positions(right, &mut ports, PortSide::West, false);
        ports
    }

    /// Java `initPositions` (the `getCardinalities=false` case; cardinalities
    /// are only needed by the greedy-switch node-switch path).
    fn init_positions(
        &mut self,
        nodes: &[LNodeId],
        ports: &mut Vec<LPortId>,
        side: PortSide,
        top_down: bool,
    ) {
        let mut num_ports = ports.len() as i32;
        let indices: Vec<usize> =
            if top_down { (0..nodes.len()).collect() } else { (0..nodes.len()).rev().collect() };
        for i in indices {
            let node = nodes[i];
            let node_ports = self.get_ports(node, side, top_down);
            for port in &node_ports {
                self.port_positions[port.0] = num_ports;
                num_ports += 1;
            }
            ports.extend(node_ports);
        }
    }

    /// Java `getPorts(node, side, topDown)`: the side's ports, reversed
    /// for (EAST & !topDown) and (WEST & topDown) so the walk stays
    /// counter-clockwise.
    fn get_ports(&self, node: LNodeId, side: PortSide, top_down: bool) -> Vec<LPortId> {
        // Sides are matched in the normalized rightward frame (NORTH→WEST,
        // SOUTH→EAST): group nodes keep NORTH/SOUTH ports because this port
        // skips ELK's import rotation (see `layer_sweep`).
        let rightward = |s: PortSide| match s {
            PortSide::North => PortSide::West,
            PortSide::South => PortSide::East,
            s => s,
        };
        let mut side_ports: Vec<LPortId> = self.arena.nodes[node.0]
            .ports
            .iter()
            .copied()
            .filter(|&p| rightward(self.arena.ports[p.0].side) == side)
            .collect();
        let reverse = match side {
            PortSide::East => !top_down,
            PortSide::West => top_down,
            _ => false,
        };
        if reverse {
            side_ports.reverse();
        }
        side_ports
    }

    // -- counting ------------------------------------------------------

    /// Java `countCrossingsOnPorts`.
    fn count_crossings_on_ports(&mut self, ports: &[LPortId]) -> i32 {
        let mut crossings = 0;
        for &port in ports {
            self.index_tree.remove_all(self.position_of(port) as usize);
            for edge in self.connected_edges(port) {
                let end_position = self.position_of(self.other_end_of(edge, port));
                if end_position > self.position_of(port) {
                    crossings += self.index_tree.rank(end_position as usize);
                    self.ends.push(end_position as usize);
                }
            }
            while let Some(e) = self.ends.pop() {
                self.index_tree.add(e);
            }
        }
        crossings
    }

    /// Java `countInLayerCrossingsOnPorts`.
    fn count_in_layer_crossings_on_ports(&mut self, ports: &[LPortId]) -> i32 {
        let mut crossings = 0;
        for &port in ports {
            self.index_tree.remove_all(self.position_of(port) as usize);
            let mut num_between_layer_edges = 0;
            for edge in self.connected_edges(port) {
                if self.is_in_layer(edge) {
                    let end_position = self.position_of(self.other_end_of(edge, port));
                    if end_position > self.position_of(port) {
                        crossings += self.index_tree.rank(end_position as usize);
                        self.ends.push(end_position as usize);
                    }
                } else {
                    num_between_layer_edges += 1;
                }
            }
            crossings += self.index_tree.size() * num_between_layer_edges;
            while let Some(e) = self.ends.pop() {
                self.index_tree.add(e);
            }
        }
        crossings
    }

    // -- helpers -------------------------------------------------------

    fn position_of(&self, port: LPortId) -> i32 {
        self.port_positions[port.0]
    }

    /// Java `LPort.getConnectedEdges()` — incoming then outgoing.
    fn connected_edges(&self, port: LPortId) -> Vec<LEdgeId> {
        self.arena.ports[port.0]
            .incoming_edges
            .iter()
            .chain(self.arena.ports[port.0].outgoing_edges.iter())
            .copied()
            .collect()
    }

    fn other_end_of(&self, edge: LEdgeId, from_port: LPortId) -> LPortId {
        let e = &self.arena.edges[edge.0];
        if Some(from_port) == e.source { e.target.unwrap() } else { e.source.unwrap() }
    }

    fn is_in_layer(&self, edge: LEdgeId) -> bool {
        let s = self.arena.edge_source_node(edge).unwrap();
        let t = self.arena.edge_target_node(edge).unwrap();
        self.arena.nodes[s.0].layer == self.arena.nodes[t.0].layer
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::graph::LGraphArena;
    use super::super::super::options::PortSide;
    use super::CrossingsCounter;

    // Two layers, each two nodes: a,b (left, a above b) and c,d (right, c
    // above d). Edges a→d and b→c cross exactly once.
    #[test]
    fn counts_one_between_layer_crossing() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let a = arena.new_node(g);
        let b = arena.new_node(g);
        let c = arena.new_node(g);
        let d = arena.new_node(g);
        for (n, l) in [(a, 0usize), (b, 0), (c, 1), (d, 1)] {
            arena.nodes[n.0].layer = Some(l);
        }

        let connect = |arena: &mut LGraphArena, src, tgt| {
            let sp = arena.new_port(src);
            arena.port_set_side(sp, PortSide::East);
            let tp = arena.new_port(tgt);
            arena.port_set_side(tp, PortSide::West);
            let e = arena.new_edge();
            arena.edge_set_source(e, Some(sp));
            arena.edge_set_target(e, Some(tp));
        };
        connect(&mut arena, a, d);
        connect(&mut arena, b, c);

        let positions = vec![0i32; arena.ports.len()];
        let mut counter = CrossingsCounter::new(&arena, positions);
        assert_eq!(counter.count_crossings_between_layers(&[a, b], &[c, d]), 1);
    }

    // Same left/right but parallel edges a→c, b→d: no crossing.
    #[test]
    fn counts_no_crossing_for_parallel_edges() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let a = arena.new_node(g);
        let b = arena.new_node(g);
        let c = arena.new_node(g);
        let d = arena.new_node(g);
        for (n, l) in [(a, 0usize), (b, 0), (c, 1), (d, 1)] {
            arena.nodes[n.0].layer = Some(l);
        }
        let connect = |arena: &mut LGraphArena, src, tgt| {
            let sp = arena.new_port(src);
            arena.port_set_side(sp, PortSide::East);
            let tp = arena.new_port(tgt);
            arena.port_set_side(tp, PortSide::West);
            let e = arena.new_edge();
            arena.edge_set_source(e, Some(sp));
            arena.edge_set_target(e, Some(tp));
        };
        connect(&mut arena, a, c);
        connect(&mut arena, b, d);

        let positions = vec![0i32; arena.ports.len()];
        let mut counter = CrossingsCounter::new(&arena, positions);
        assert_eq!(counter.count_crossings_between_layers(&[a, b], &[c, d]), 0);
    }
}
