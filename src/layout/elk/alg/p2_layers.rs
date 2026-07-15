//! Port of `org.eclipse.elk.alg.layered.p2layers.NetworkSimplexLayerer`
//! (phase 2, the default NETWORK_SIMPLEX layering strategy).
//!
//! Splits the graph into connected components (largest-ish first —
//! Java's quirky ordering compares only against the current first
//! element), runs the network simplex per component with balancing
//! enabled, and materializes `Layer`s on the LGraph. The component
//! interplay happens through `previousLayeringNodeCounts`, which biases
//! the balancing of later components toward less-filled layers.

use super::graph::{LGraphArena, LGraphId, LNodeId};
use super::network_simplex::{NetworkSimplex, NGraph};

const ITER_LIMIT_FACTOR: usize = 4;

/// Java `NetworkSimplexLayerer.process(graph)`.
pub fn layer_nodes(arena: &mut LGraphArena, graph: LGraphId) {
    let thoroughness = arena.graphs[graph.0].props.thoroughness as usize * ITER_LIMIT_FACTOR;
    let the_nodes: Vec<LNodeId> = arena.graphs[graph.0].layerless_nodes.clone();
    if the_nodes.is_empty() {
        return;
    }

    let components = connected_components(arena, &the_nodes);

    let mut previous_layering_node_counts: Option<Vec<i32>> = None;
    for conn_comp in &components {
        let iter_limit = thoroughness * (conn_comp.len() as f64).sqrt() as usize;
        let mut ngraph = initialize(arena, conn_comp);
        NetworkSimplex::for_graph(&mut ngraph)
            .with_iteration_limit(iter_limit)
            .with_previous_layering(previous_layering_node_counts.take())
            .with_balancing(true)
            .execute();

        for nnode in &ngraph.nodes {
            let layer = nnode.layer as usize;
            while arena.graphs[graph.0].layers.len() <= layer {
                let at = arena.graphs[graph.0].layers.len();
                arena.insert_layer(graph, at);
            }
            let lnode = LNodeId(nnode.origin.expect("layerer NNode without origin"));
            arena.node_set_layer(graph, lnode, Some(layer));
        }

        if components.len() > 1 {
            previous_layering_node_counts = Some(
                arena.graphs[graph.0].layers.iter().map(|l| l.nodes.len() as i32).collect(),
            );
        }
    }
    arena.graphs[graph.0].layerless_nodes.clear();
}

/// Java `connectedComponents(theNodes)` — DFS over ports' connected
/// edges; component list ordering keeps Java's exact quirk.
fn connected_components(arena: &LGraphArena, the_nodes: &[LNodeId]) -> Vec<Vec<LNodeId>> {
    let mut visited: std::collections::HashSet<LNodeId> = Default::default();
    let node_set: std::collections::HashSet<LNodeId> = the_nodes.iter().copied().collect();
    let mut components: std::collections::VecDeque<Vec<LNodeId>> = Default::default();
    for &node in the_nodes {
        if !visited.contains(&node) {
            let mut component = Vec::new();
            dfs(arena, node, &node_set, &mut visited, &mut component);
            if components.is_empty() || components.front().unwrap().len() < component.len() {
                components.push_front(component);
            } else {
                components.push_back(component);
            }
        }
    }
    components.into()
}

fn dfs(
    arena: &LGraphArena,
    node: LNodeId,
    node_set: &std::collections::HashSet<LNodeId>,
    visited: &mut std::collections::HashSet<LNodeId>,
    out: &mut Vec<LNodeId>,
) {
    visited.insert(node);
    out.push(node);
    for &port in &arena.nodes[node.0].ports {
        let edges: Vec<_> = arena.ports[port.0]
            .incoming_edges
            .iter()
            .chain(arena.ports[port.0].outgoing_edges.iter())
            .copied()
            .collect();
        for edge in edges {
            let src = arena.edges[edge.0].source.unwrap();
            let tgt = arena.edges[edge.0].target.unwrap();
            let opposite_port = if src == port { tgt } else { src };
            let opposite = arena.ports[opposite_port.0].owner.unwrap();
            if node_set.contains(&opposite) && !visited.contains(&opposite) {
                dfs(arena, opposite, node_set, visited, out);
            }
        }
    }
}

/// Java `initialize(theNodes)` — build the NGraph: weight =
/// `max(1, priority.shortness)` (never set in scope → 1), delta 1,
/// self-loops skipped.
fn initialize(arena: &LGraphArena, the_nodes: &[LNodeId]) -> NGraph {
    let mut ngraph = NGraph::default();
    let mut node_map: std::collections::HashMap<LNodeId, usize> = Default::default();
    for &lnode in the_nodes {
        let n = ngraph.add_node(Some(lnode.0));
        node_map.insert(lnode, n);
    }
    for &lnode in the_nodes {
        for ledge in arena.node_outgoing_edges(lnode) {
            if arena.edge_is_self_loop(ledge) {
                continue;
            }
            let src = arena.edge_source_node(ledge).unwrap();
            let tgt = arena.edge_target_node(ledge).unwrap();
            ngraph.add_edge(node_map[&src], node_map[&tgt], 1.0, 1);
        }
    }
    ngraph
}
