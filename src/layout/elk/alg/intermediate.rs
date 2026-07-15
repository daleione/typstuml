//! Ports of the ELK `layered` **intermediate** processors that run
//! between phases (EPL-2.0, see `LICENSE.md`). Upstream:
//! `org.eclipse.elk.alg.layered.intermediate`.
//!
//! This file grows one processor at a time as the phase pipeline is
//! filled in. Currently ported: `LongEdgeSplitter` (before phase 3).

use super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId, LPortId, NodeType};
use super::options::{PortConstraints, PortSide};

/// Port of
/// `org.eclipse.elk.alg.layered.intermediate.LongEdgeSplitter` (before
/// phase 3). Splits every edge spanning more than one layer into a
/// chain of `LONG_EDGE` dummy nodes so the layering becomes *proper*
/// (all edges connect subsequent layers) — the precondition of the
/// crossing-minimization phase.
///
/// Scope: `EDGE_THICKNESS` is never set in draw-uml (default → 0, so
/// dummy height and port offset are 0) and flat/architecture edges
/// carry no labels (`moveHeadLabels` is asserted away). The `LABEL`
/// dummy branches of `setDummyNodeProperties` therefore never fire; the
/// `LONG_EDGE` branch does (a cascaded split feeds a dummy back in).
pub fn long_edge_splitter(arena: &mut LGraphArena, graph: LGraphId) {
    let layer_count = arena.graphs[graph.0].layers.len();
    if layer_count <= 2 {
        return;
    }

    // Java advances a ListIterator over consecutive layer pairs
    // (layer, nextLayer). The layer *count* is stable (splits only add
    // nodes to existing layers), so iterating indices 0..count-1 with a
    // fresh snapshot of each layer's nodes is equivalent — dummies added
    // to layer li+1 while processing li are picked up when li+1 becomes
    // the current layer.
    for li in 0..layer_count - 1 {
        let nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        for node in nodes {
            for port in arena.nodes[node.0].ports.clone() {
                for edge in arena.ports[port.0].outgoing_edges.clone() {
                    let target_node = arena.edge_target_node(edge).unwrap();
                    let target_layer = arena.nodes[target_node.0].layer.unwrap();
                    if target_layer != li && target_layer != li + 1 {
                        let dummy = create_dummy_node(arena, graph, li + 1, edge);
                        split_edge(arena, edge, dummy);
                    }
                }
            }
        }
    }
}

/// Java `LongEdgeSplitter.createDummyNode`: a `LONG_EDGE` dummy with
/// FIXED_POS ports, placed in `target_layer`, remembering the split
/// edge as its `ORIGIN`.
fn create_dummy_node(
    arena: &mut LGraphArena,
    graph: LGraphId,
    target_layer: usize,
    edge_to_split: LEdgeId,
) -> LNodeId {
    let dummy = arena.new_node(graph);
    // Java `new LNode(graph)` does not touch layerless_nodes; the arena's
    // new_node appends there, so undo it — the node lives in a layer.
    arena.graphs[graph.0].layerless_nodes.pop();
    arena.nodes[dummy.0].node_type = NodeType::LongEdge;
    arena.nodes[dummy.0].props.origin_edge = Some(edge_to_split);
    arena.nodes[dummy.0].props.port_constraints = PortConstraints::FixedPos;
    arena.node_set_layer(graph, dummy, Some(target_layer));
    dummy
}

/// Java `LongEdgeSplitter.splitEdge` (the public static utility). Adds a
/// WEST input + EAST output port to `dummy_node`, reroutes `edge` into
/// the input, and creates the continuation edge from the output to the
/// old target.
pub fn split_edge(arena: &mut LGraphArena, edge: LEdgeId, dummy_node: LNodeId) -> LEdgeId {
    let old_edge_target = arena.edges[edge.0].target.unwrap();

    // EDGE_THICKNESS is never negative-defaulted in scope → thickness 0,
    // port offset 0, dummy height 0.
    assert!(
        arena.edges[edge.0].labels.is_empty(),
        "edge labels are outside the current flat/architecture scope"
    );

    // EDGE_THICKNESS defaults to 1 and no draw-uml edge overrides it, so
    // the dummy is 1 unit tall in the layer direction and its ports sit
    // at floor(1/2) = 0 (KVector default).
    arena.nodes[dummy_node.0].size.y = 1.0;

    let dummy_input = arena.new_port(dummy_node);
    arena.port_set_side(dummy_input, PortSide::West);
    let dummy_output = arena.new_port(dummy_node);
    arena.port_set_side(dummy_output, PortSide::East);

    arena.edge_set_target(edge, Some(dummy_input));

    let dummy_edge = arena.new_edge();
    arena.edges[dummy_edge.0].props = arena.edges[edge.0].props.clone();
    arena.edge_set_source(dummy_edge, Some(dummy_output));
    arena.edge_set_target(dummy_edge, Some(old_edge_target));

    set_dummy_node_properties(arena, dummy_node, edge, dummy_edge);
    dummy_edge
}

/// Java `LongEdgeSplitter.setDummyNodeProperties`: carry the original
/// edge's real source/target ports through the dummy chain. In scope
/// only the `LONG_EDGE`-source and the plain `else` branches occur
/// (LABEL dummies don't exist in flat/architecture inputs).
fn set_dummy_node_properties(
    arena: &mut LGraphArena,
    dummy_node: LNodeId,
    in_edge: LEdgeId,
    out_edge: LEdgeId,
) {
    let in_source_node = arena.edge_source_node(in_edge).unwrap();
    let out_target_node = arena.edge_target_node(out_edge).unwrap();
    assert!(
        arena.nodes[in_source_node.0].node_type != NodeType::Label
            && arena.nodes[out_target_node.0].node_type != NodeType::Label,
        "label dummies are outside the current scope"
    );

    if arena.nodes[in_source_node.0].node_type == NodeType::LongEdge {
        arena.nodes[dummy_node.0].props.long_edge_source =
            arena.nodes[in_source_node.0].props.long_edge_source;
        arena.nodes[dummy_node.0].props.long_edge_target =
            arena.nodes[in_source_node.0].props.long_edge_target;
    } else {
        arena.nodes[dummy_node.0].props.long_edge_source = arena.edges[in_edge.0].source;
        arena.nodes[dummy_node.0].props.long_edge_target = arena.edges[out_edge.0].target;
    }
}

// ----------------------------------------------------------------------
// PortSideProcessor (before phase 3)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.PortSideProcessor`
/// in its **before-phase-3** slot: after cycle breaking, so no inverted
/// ports occur. Free-port nodes get input ports on the WEST, output
/// ports on the EAST (the algorithm's normalized rightward orientation —
/// the user's `direction` is applied only at coordinate transfer), and
/// their constraints tighten to FIXED_SIDE. Side-fixed nodes only get
/// UNDEFINED-side ports assigned (LONG_EDGE dummies already have WEST/EAST
/// from the splitter, so they are untouched).
pub fn port_side_processor(arena: &mut LGraphArena, graph: LGraphId) {
    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            process_node_port_sides(arena, node);
        }
    }
}

fn process_node_port_sides(arena: &mut LGraphArena, node: LNodeId) {
    if arena.nodes[node.0].props.port_constraints.is_side_fixed() {
        for port in arena.nodes[node.0].ports.clone() {
            if arena.ports[port.0].side == PortSide::Undefined {
                set_port_side(arena, port);
            }
        }
    } else {
        for port in arena.nodes[node.0].ports.clone() {
            set_port_side(arena, port);
        }
        arena.nodes[node.0].props.port_constraints = PortConstraints::FixedSide;
    }
}

/// Java `PortSideProcessor.setPortSide`. `PORT_DUMMY` (external ports)
/// never occurs on the flat path in scope; `netFlow = incoming −
/// outgoing`, so an output port (net flow < 0) goes EAST, an input port
/// WEST.
fn set_port_side(arena: &mut LGraphArena, port: LPortId) {
    if let Some(port_dummy) = arena.ports[port.0].props.port_dummy {
        let side = arena.nodes[port_dummy.0].props.ext_port_side;
        arena.port_set_side(port, side);
    } else {
        let net_flow = arena.ports[port.0].incoming_edges.len() as i32
            - arena.ports[port.0].outgoing_edges.len() as i32;
        let side = if net_flow < 0 { PortSide::East } else { PortSide::West };
        arena.port_set_side(port, side);
    }
}

// ----------------------------------------------------------------------
// InvertedPortProcessor (before phase 3, after PortSideProcessor)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.InvertedPortProcessor`
/// (configured unconditionally by the orthogonal edge router, between
/// `PortSideProcessor` and `PortListSorter`). An edge that *arrives* at an
/// EAST-side port or *leaves* a WEST-side port flows against the layer
/// direction at that node; a same-layer `LONG_EDGE` dummy is inserted so
/// the edge can route around the node. Free-port graphs never trigger it
/// (netFlow side assignment can't invert), which is why the flat corpora
/// were byte-exact without it — but a compound root can: cycle breaking
/// reverses an edge at a group node whose ports were FIXED by the
/// compound preprocessor.
///
/// Sides are matched in the normalized rightward frame (NORTH→WEST,
/// SOUTH→EAST) because this port skips ELK's `TO_INTERNAL_LTR` import
/// rotation, leaving group-node ports on NORTH/SOUTH (see
/// `p3order::layer_sweep`). The dummy's own ports are algorithm-internal
/// and created WEST/EAST directly, like the splitter's.
pub fn inverted_port_processor(arena: &mut LGraphArena, graph: LGraphId) {
    let rightward = |side: PortSide| match side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    };
    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        // Dummies created for this layer are collected and appended after
        // the layer's own nodes have been processed (Java defers via
        // `unassignedNodes` to dodge concurrent modification; the net
        // list position — end of the same layer — is identical).
        let mut unassigned: Vec<LNodeId> = Vec::new();
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            if arena.nodes[node.0].node_type != NodeType::Normal {
                continue;
            }
            if !arena.nodes[node.0].props.port_constraints.is_side_fixed() {
                continue;
            }
            // Input ports on the right side.
            for port in arena.nodes[node.0].ports.clone() {
                if rightward(arena.ports[port.0].side) != PortSide::East
                    || arena.ports[port.0].incoming_edges.is_empty()
                {
                    continue;
                }
                for edge in arena.ports[port.0].incoming_edges.clone() {
                    create_east_port_side_dummies(arena, graph, port, edge, &mut unassigned);
                }
            }
            // Output ports on the left side.
            for port in arena.nodes[node.0].ports.clone() {
                if rightward(arena.ports[port.0].side) != PortSide::West
                    || arena.ports[port.0].outgoing_edges.is_empty()
                {
                    continue;
                }
                for edge in arena.ports[port.0].outgoing_edges.clone() {
                    create_west_port_side_dummies(arena, graph, port, edge, &mut unassigned);
                }
            }
        }
        for dummy in unassigned {
            arena.node_set_layer(graph, dummy, Some(li));
        }
    }
}

/// Shared body of `createEast/WestPortSideDummies`: the same-layer
/// `LONG_EDGE` dummy with a WEST input + EAST output port, `ORIGIN` set
/// to the rerouted edge, FIXED_POS constraints.
fn create_inverted_port_dummy(
    arena: &mut LGraphArena,
    graph: LGraphId,
    edge: LEdgeId,
    unassigned: &mut Vec<LNodeId>,
) -> (LNodeId, LPortId, LPortId) {
    let dummy = arena.new_node(graph);
    arena.graphs[graph.0].layerless_nodes.pop();
    arena.nodes[dummy.0].node_type = NodeType::LongEdge;
    arena.nodes[dummy.0].props.origin_edge = Some(edge);
    arena.nodes[dummy.0].props.port_constraints = PortConstraints::FixedPos;
    unassigned.push(dummy);

    let dummy_input = arena.new_port(dummy);
    arena.port_set_side(dummy_input, PortSide::West);
    let dummy_output = arena.new_port(dummy);
    arena.port_set_side(dummy_output, PortSide::East);
    (dummy, dummy_input, dummy_output)
}

/// Java `InvertedPortProcessor.createEastPortSideDummies`: reroute an
/// edge arriving at an east-side input port through a same-layer dummy;
/// the dummy→port connection becomes an in-layer edge.
fn create_east_port_side_dummies(
    arena: &mut LGraphArena,
    graph: LGraphId,
    eastward_port: LPortId,
    edge: LEdgeId,
    unassigned: &mut Vec<LNodeId>,
) {
    debug_assert_eq!(arena.edges[edge.0].target, Some(eastward_port));
    // Ignore self loops.
    if arena.edge_source_node(edge) == arena.ports[eastward_port.0].owner {
        return;
    }
    assert!(
        arena.edges[edge.0].labels.is_empty(),
        "edge labels are outside the current flat/architecture scope"
    );

    let (dummy, dummy_input, dummy_output) =
        create_inverted_port_dummy(arena, graph, edge, unassigned);

    // Reroute the original edge and connect the dummy to the odd port.
    arena.edge_set_target(edge, Some(dummy_input));
    let dummy_edge = arena.new_edge();
    arena.edges[dummy_edge.0].props = arena.edges[edge.0].props.clone();
    arena.edge_set_source(dummy_edge, Some(dummy_output));
    arena.edge_set_target(dummy_edge, Some(eastward_port));

    set_long_edge_source_and_target(arena, dummy, dummy_input, dummy_output);
}

/// Java `InvertedPortProcessor.createWestPortSideDummies`: reroute an
/// edge leaving a west-side output port through a same-layer dummy; the
/// port→dummy connection becomes an in-layer edge.
fn create_west_port_side_dummies(
    arena: &mut LGraphArena,
    graph: LGraphId,
    westward_port: LPortId,
    edge: LEdgeId,
    unassigned: &mut Vec<LNodeId>,
) {
    debug_assert_eq!(arena.edges[edge.0].source, Some(westward_port));
    // Ignore self loops.
    if arena.edge_target_node(edge) == arena.ports[westward_port.0].owner {
        return;
    }
    assert!(
        arena.edges[edge.0].labels.is_empty(),
        "edge labels are outside the current flat/architecture scope"
    );

    let (dummy, dummy_input, dummy_output) =
        create_inverted_port_dummy(arena, graph, edge, unassigned);

    // Reroute the original edge into the dummy and continue from the
    // dummy to the original target.
    let original_target = arena.edges[edge.0].target;
    arena.edge_set_target(edge, Some(dummy_input));
    let dummy_edge = arena.new_edge();
    arena.edges[dummy_edge.0].props = arena.edges[edge.0].props.clone();
    arena.edge_set_source(dummy_edge, Some(dummy_output));
    arena.edge_set_target(dummy_edge, original_target);

    set_long_edge_source_and_target(arena, dummy, dummy_input, dummy_output);
}

/// Java `InvertedPortProcessor.setLongEdgeSourceAndTarget`: carry the
/// original endpoints through the new dummy, chaining through existing
/// `LONG_EDGE` dummies on either side.
fn set_long_edge_source_and_target(
    arena: &mut LGraphArena,
    dummy: LNodeId,
    dummy_input: LPortId,
    dummy_output: LPortId,
) {
    let source_port = arena.edges[arena.ports[dummy_input.0].incoming_edges[0].0].source.unwrap();
    let source_node = arena.ports[source_port.0].owner.unwrap();
    let target_port = arena.edges[arena.ports[dummy_output.0].outgoing_edges[0].0].target.unwrap();
    let target_node = arena.ports[target_port.0].owner.unwrap();

    if arena.nodes[source_node.0].node_type == NodeType::LongEdge {
        arena.nodes[dummy.0].props.long_edge_source =
            arena.nodes[source_node.0].props.long_edge_source;
    } else {
        arena.nodes[dummy.0].props.long_edge_source = Some(source_port);
    }
    if arena.nodes[target_node.0].node_type == NodeType::LongEdge {
        arena.nodes[dummy.0].props.long_edge_target =
            arena.nodes[target_node.0].props.long_edge_target;
    } else {
        arena.nodes[dummy.0].props.long_edge_target = Some(target_port);
    }
}

// ----------------------------------------------------------------------
// PortListSorter (before phase 3)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.PortListSorter`.
/// Sorts each node's port list "clockwise from the leftmost northern
/// port" so the crossing-min port-rank calculation is well defined.
/// `PORT_SORTING_STRATEGY` defaults to INPUT_ORDER, so the PORT_DEGREE
/// branch is never taken in scope.
///
/// Two paths, matching Java:
/// - order-fixed nodes (FIXED_POS LONG_EDGE dummies): sort by side then
///   by position within a side (`CMP_COMBINED`).
/// - side-fixed nodes (FIXED_SIDE real nodes): stable sort by side
///   (`CMP_PORT_SIDE`, preserving input order within a side) then
///   reverse the SOUTH and WEST sub-ranges — bug-for-bug with ELK's
///   `findPortSideRange`/`reverse` (see comments).
///
/// Sides are compared in the normalized rightward frame (NORTH→WEST,
/// SOUTH→EAST): ELK sorts *after* its `TO_INTERNAL_LTR` import rotation,
/// which this port skips, leaving group-node ports on NORTH/SOUTH. In
/// that frame a group node's outputs (SOUTH→EAST) sort before its inputs
/// (NORTH→WEST) and only the inputs are reversed — matching elkjs's
/// rotated W/E lists. W/E-only nodes (all flat corpora) are unaffected.
pub fn port_list_sorter(arena: &mut LGraphArena, graph: LGraphId) {
    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            let pc = arena.nodes[node.0].props.port_constraints;
            let mut ports = arena.nodes[node.0].ports.clone();
            if pc.is_order_fixed() {
                ports.sort_by(|&a, &b| cmp_combined(arena, a, b));
            } else if pc.is_side_fixed() {
                ports.sort_by(|&a, &b| side_ordinal(arena, a).cmp(&side_ordinal(arena, b)));
                reverse_west_and_south_side(arena, &mut ports);
            }
            arena.nodes[node.0].ports = ports;
        }
    }
}

fn side_ordinal(arena: &LGraphArena, port: LPortId) -> i32 {
    normalized_side(arena, port) as i32
}

/// The port's side in the normalized rightward frame (NORTH→WEST,
/// SOUTH→EAST) — see [`port_list_sorter`].
fn normalized_side(arena: &LGraphArena, port: LPortId) -> PortSide {
    match arena.ports[port.0].side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    }
}

/// Java `CMP_COMBINED` = `CMP_PORT_SIDE.thenComparing(CMP_FIXED_ORDER_AND_FIXED_POS)`.
/// In scope this only runs on FIXED_POS dummies, so the FIXED_ORDER
/// PORT_INDEX branch is irrelevant; ties within a side break by position
/// per side (N: x asc, E: y asc, S: x desc, W: y desc).
fn cmp_combined(arena: &LGraphArena, p1: LPortId, p2: LPortId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let so = side_ordinal(arena, p1).cmp(&side_ordinal(arena, p2));
    if so != Ordering::Equal {
        return so;
    }
    let (a, b) = (&arena.ports[p1.0], &arena.ports[p2.0]);
    match normalized_side(arena, p1) {
        PortSide::North => a.position.x.total_cmp(&b.position.x),
        PortSide::East => a.position.y.total_cmp(&b.position.y),
        PortSide::South => b.position.x.total_cmp(&a.position.x),
        PortSide::West => b.position.y.total_cmp(&a.position.y),
        PortSide::Undefined => Ordering::Equal,
    }
}

/// Java `PortListSorter.reverseWestAndSouthSide` (+ its private
/// `findPortSideRange`/`reverse`), ported verbatim including the
/// `ports.get(lowIdx)` re-read in the second while loop and the
/// `highIdx <= lowIdx + 2` early return.
fn reverse_west_and_south_side(arena: &LGraphArena, ports: &mut [LPortId]) {
    if ports.len() <= 1 {
        return;
    }
    let (s_lo, s_hi) = find_port_side_range(arena, ports, PortSide::South);
    reverse_range(ports, s_lo, s_hi);
    let (w_lo, w_hi) = find_port_side_range(arena, ports, PortSide::West);
    reverse_range(ports, w_lo, w_hi);
}

fn find_port_side_range(
    arena: &LGraphArena,
    ports: &[LPortId],
    side: PortSide,
) -> (usize, usize) {
    if ports.is_empty() {
        return (0, 0);
    }
    let ord = |p: LPortId| side_ordinal(arena, p);
    let mut current_side = ord(ports[0]);
    let mut low_idx = 0usize;
    let lb = side as i32;
    let hb = side as i32 + 1;
    while low_idx < ports.len() - 1 && current_side < lb {
        low_idx += 1;
        current_side = ord(ports[low_idx]);
    }
    let mut high_idx = low_idx;
    // NB: Java re-reads ports.get(lowIdx) here (not highIdx), so
    // current_side is effectively frozen at ports[low_idx] — replicated.
    while high_idx < ports.len() - 1 && current_side < hb {
        high_idx += 1;
        current_side = ord(ports[low_idx]);
    }
    (low_idx, high_idx)
}

fn reverse_range(ports: &mut [LPortId], low_idx: usize, high_idx: usize) {
    if high_idx <= low_idx + 2 {
        return;
    }
    let n = (high_idx - low_idx) / 2;
    for i in 0..n {
        ports.swap(low_idx + i, high_idx - i - 1);
    }
}

// ----------------------------------------------------------------------
// LongEdgeJoiner (after phase 5)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.LongEdgeJoiner`.
/// Reassembles every `LONG_EDGE` dummy chain back into the original edge:
/// the first (retained) edge inherits the last dropped edge's target and
/// concatenates the dropped edges' bend points, so the surviving edge
/// carries the full source→target polyline. Dummies are removed from
/// their layers.
///
/// Scope: `UNNECESSARY_BENDPOINTS` is never set by draw-uml (default
/// `false`), so no extra bend point is added at the dummy position;
/// edges carry no labels and no junction points in the flat/architecture
/// scope (asserted / skipped), and each dummy carries exactly one edge.
pub fn long_edge_joiner(arena: &mut LGraphArena, graph: LGraphId) {
    for li in 0..arena.graphs[graph.0].layers.len() {
        let nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        for node in nodes {
            if arena.nodes[node.0].node_type == NodeType::LongEdge {
                join_at(arena, node);
                arena.node_set_layer(graph, node, None);
            }
        }
    }
}

/// Java `LongEdgeJoiner.joinAt` (with `addUnnecessaryBendpoints = false`).
fn join_at(arena: &mut LGraphArena, dummy: LNodeId) {
    let west = arena.nodes[dummy.0]
        .ports
        .iter()
        .copied()
        .find(|&p| arena.ports[p.0].side == PortSide::West)
        .expect("long-edge dummy has a WEST port");
    let east = arena.nodes[dummy.0]
        .ports
        .iter()
        .copied()
        .find(|&p| arena.ports[p.0].side == PortSide::East)
        .expect("long-edge dummy has an EAST port");

    let mut edge_count = arena.ports[west.0].incoming_edges.len();
    while edge_count > 0 {
        edge_count -= 1;
        let surviving = arena.ports[west.0].incoming_edges[0];
        let dropped = arena.ports[east.0].outgoing_edges[0];

        // Re-target the surviving edge at the dropped edge's target,
        // preserving the target port's incoming-edge index (KIPRA-1670).
        let dropped_target = arena.edges[dropped.0].target.unwrap();
        let idx = arena.ports[dropped_target.0]
            .incoming_edges
            .iter()
            .position(|&e| e == dropped)
            .unwrap();
        arena.edge_set_source(dropped, None);
        arena.edge_set_target(dropped, None);
        arena.edge_set_target_at_index(surviving, dropped_target, idx);

        // Concatenate the dropped edge's bend points onto the survivor.
        let dropped_bends = arena.edges[dropped.0].bend_points.clone();
        arena.edges[surviving.0].bend_points.extend(dropped_bends);
    }
}

// ----------------------------------------------------------------------
// ReversedEdgeRestorer (after phase 5)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.ReversedEdgeRestorer`.
/// Every edge the cycle breaker reversed is flipped back to its original
/// direction (`edge.reverse(graph, false)`), which also reverses its bend
/// point chain so it reads source→target again.
pub fn reversed_edge_restorer(arena: &mut LGraphArena, graph: LGraphId) {
    for li in 0..arena.graphs[graph.0].layers.len() {
        let nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        for node in nodes {
            for port in arena.nodes[node.0].ports.clone() {
                for edge in arena.ports[port.0].outgoing_edges.clone() {
                    if arena.edges[edge.0].props.reversed {
                        arena.edge_reverse(edge, false);
                    }
                }
            }
        }
    }
}
