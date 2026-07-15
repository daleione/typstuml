//! Port of the ELK `layered` layer-constraint intermediate processors
//! `LayerConstraintPreprocessor` / `LayerConstraintPostprocessor` (EPL-2.0,
//! see `elk/LICENSE.md`), restricted to the `FIRST_SEPARATE` /
//! `LAST_SEPARATE` path that external-port dummies use.
//!
//! Before layering, [`preprocess`] **hides** every `*_SEPARATE` node
//! (external-port dummy): it disconnects each incident edge (nulling the
//! far endpoint, remembering it on the edge) and removes the node from
//! the layerless set, so the network simplex lays out only the real
//! nodes. After layering, [`postprocess`] restores the hidden nodes into
//! freshly created first/last boundary layers and reconnects their edges.
//!
//! Hiding the external dummies is exactly what makes a nested graph's
//! *real-node* layering match elkjs (the external edges no longer pull
//! nodes across layers), and it gives every external-port dummy its own
//! dedicated boundary layer.
//!
//! Scope: only `*_SEPARATE` nodes occur (no real-node FIRST/LAST layer
//! constraints, no label dummies). If a real node would inherit a
//! FIRST/LAST constraint (it is connected *only* to hidden dummies of one
//! kind), that is asserted against — the benchmark never triggers it, and
//! honouring it needs the network-simplex constraint machinery we have
//! not ported.

use super::graph::{LGraphArena, LGraphId, LNodeId};
use super::options::LayerConstraint;

/// Java `LayerConstraintPreprocessor.process`. Returns the hidden nodes
/// (to feed [`postprocess`]).
pub fn preprocess(arena: &mut LGraphArena, graph: LGraphId) -> Vec<LNodeId> {
    let mut hidden = Vec::new();
    // Track, per opposite (real) node, which kinds of hidden node it is
    // connected to (Java `HIDDEN_NODE_CONNECTIONS`).
    let mut connections: std::collections::HashMap<LNodeId, LayerConstraint> =
        std::collections::HashMap::new();

    for node in arena.graphs[graph.0].layerless_nodes.clone() {
        let lc = arena.nodes[node.0].props.layer_constraint;
        if lc != LayerConstraint::FirstSeparate && lc != LayerConstraint::LastSeparate {
            continue;
        }
        ensure_no_inacceptable_edges(arena, node, lc);
        hide(arena, node, lc, &mut connections);
        hidden.push(node);
    }

    arena.graphs[graph.0].layerless_nodes.retain(|n| !hidden.contains(n));
    hidden
}

fn ensure_no_inacceptable_edges(arena: &LGraphArena, node: LNodeId, lc: LayerConstraint) {
    match lc {
        LayerConstraint::FirstSeparate => {
            assert!(
                arena.node_incoming_edges(node).is_empty(),
                "FIRST_SEPARATE node must have no incoming edges (needs EdgeAndLayerConstraintEdgeReverser)"
            );
        }
        LayerConstraint::LastSeparate => {
            assert!(
                arena.node_outgoing_edges(node).is_empty(),
                "LAST_SEPARATE node must have no outgoing edges (needs EdgeAndLayerConstraintEdgeReverser)"
            );
        }
        _ => {}
    }
}

fn hide(
    arena: &mut LGraphArena,
    node: LNodeId,
    lc: LayerConstraint,
    connections: &mut std::collections::HashMap<LNodeId, LayerConstraint>,
) {
    for edge in arena.node_connected_edges(node) {
        let src_node = arena.edge_source_node(edge);
        let is_outgoing = src_node == Some(node);
        let opposite_port =
            if is_outgoing { arena.edges[edge.0].target.unwrap() } else { arena.edges[edge.0].source.unwrap() };
        let opposite_node = arena.ports[opposite_port.0].owner.unwrap();

        if is_outgoing {
            arena.edge_set_target(edge, None);
        } else {
            arena.edge_set_source(edge, None);
        }
        arena.edges[edge.0].props.original_opposite_port = Some(opposite_port);

        update_opposite_node_layer_constraints(arena, opposite_node, lc, connections);
    }
}

/// Java `updateOppositeNodeLayerConstraints`: track which kinds of hidden
/// node the opposite node connects to; once it is fully disconnected and
/// was tied to exactly one kind, it inherits FIRST (from FIRST_SEPARATE)
/// or LAST (from LAST_SEPARATE).
fn update_opposite_node_layer_constraints(
    arena: &mut LGraphArena,
    opposite: LNodeId,
    hidden_lc: LayerConstraint,
    connections: &mut std::collections::HashMap<LNodeId, LayerConstraint>,
) {
    if arena.nodes[opposite.0].props.layer_constraint != LayerConstraint::None {
        return;
    }
    let prev = connections.get(&opposite).copied().unwrap_or(LayerConstraint::None);
    let combined = combine(prev, hidden_lc);
    connections.insert(opposite, combined);

    // Still connected to something? then no constraint yet.
    if !arena.node_connected_edges(opposite).is_empty() {
        return;
    }
    arena.nodes[opposite.0].props.layer_constraint = match combined {
        LayerConstraint::FirstSeparate => LayerConstraint::First,
        LayerConstraint::LastSeparate => LayerConstraint::Last,
        _ => LayerConstraint::None,
    };
}

/// Java `moveFirstAndLastNodes` (no-label subset): move every real node
/// carrying a FIRST/LAST constraint (inherited above) into the existing
/// first/last real layer, then drop the layers left empty. Such nodes are
/// edge-free after hiding, so the "no incoming/outgoing" preconditions
/// hold trivially.
fn move_first_and_last_nodes(arena: &mut LGraphArena, graph: LGraphId) {
    let n_layers = arena.graphs[graph.0].layers.len();
    if n_layers == 0 {
        return;
    }
    let last = n_layers - 1;
    let all_nodes: Vec<LNodeId> =
        arena.graphs[graph.0].layers.iter().flat_map(|l| l.nodes.clone()).collect();
    for node in all_nodes {
        match arena.nodes[node.0].props.layer_constraint {
            LayerConstraint::First => {
                assert!(arena.node_incoming_edges(node).is_empty(), "FIRST node has incoming edges");
                arena.node_set_layer(graph, node, Some(0));
            }
            LayerConstraint::Last => {
                assert!(arena.node_outgoing_edges(node).is_empty(), "LAST node has outgoing edges");
                arena.node_set_layer(graph, node, Some(last));
            }
            _ => {}
        }
    }
    arena.remove_empty_layers(graph);
}

/// Java `HiddenNodeConnections.combine`: NONE→x; same→same; different→a
/// sentinel meaning "both" (represented here as `None`, i.e. no
/// constraint should be inferred).
fn combine(prev: LayerConstraint, add: LayerConstraint) -> LayerConstraint {
    match prev {
        LayerConstraint::None => add,
        p if p == add => p,
        _ => LayerConstraint::None, // connected to both kinds → no constraint
    }
}

/// Java `LayerConstraintPostprocessor.process` (the `HIDDEN_NODES`
/// restore half only). Creates a first and last boundary layer, drops
/// each hidden `*_SEPARATE` node into the appropriate one, and reconnects
/// its edges.
pub fn postprocess(arena: &mut LGraphArena, graph: LGraphId, hidden: &[LNodeId]) {
    if hidden.is_empty() {
        return;
    }
    move_first_and_last_nodes(arena, graph);

    let mut first_nodes: Vec<LNodeId> = Vec::new();
    let mut last_nodes: Vec<LNodeId> = Vec::new();
    for &node in hidden {
        match arena.nodes[node.0].props.layer_constraint {
            LayerConstraint::FirstSeparate => first_nodes.push(node),
            LayerConstraint::LastSeparate => last_nodes.push(node),
            other => panic!("only *_SEPARATE nodes are hidden, got {other:?}"),
        }
        // Reconnect this node's half-hidden edges.
        for edge in arena.node_connected_edges(node) {
            if arena.edges[edge.0].source.is_some() && arena.edges[edge.0].target.is_some() {
                continue; // already restored (edge between two hidden nodes)
            }
            let is_outgoing = arena.edges[edge.0].target.is_none();
            let opp = arena.edges[edge.0].props.original_opposite_port.unwrap();
            if is_outgoing {
                arena.edge_set_target(edge, Some(opp));
            } else {
                arena.edge_set_source(edge, Some(opp));
            }
        }
    }

    // Append the last-separate layer, prepend the first-separate layer.
    if !last_nodes.is_empty() {
        let at = arena.graphs[graph.0].layers.len();
        arena.graphs[graph.0].layers.push(super::graph::Layer::default());
        for n in last_nodes {
            arena.node_set_layer(graph, n, Some(at));
        }
    }
    if !first_nodes.is_empty() {
        arena.insert_layer(graph, 0);
        for n in first_nodes {
            arena.node_set_layer(graph, n, Some(0));
        }
    }
}
