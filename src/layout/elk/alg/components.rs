//! Port of
//! `org.eclipse.elk.alg.layered.components.ComponentsProcessor.split`
//! (+ its private DFS component collector).
//!
//! ELK runs this at the top of the **flat** layout path
//! (`ElkLayered.doLayout`): it splits a graph into its connected
//! components, lays each out independently, then `combine`s them.
//! Crucially it is NOT run on the hierarchical path
//! (`ElkLayered.doCompoundLayout`) — so callers must only invoke it for
//! non-`INCLUDE_CHILDREN` graphs.
//!
//! Even for a single connected component, split matters: the DFS
//! rebuilds `layerlessNodes` in discovery order, and that order is what
//! the greedy cycle breaker (phase 1) consumes. Skipping split lets the
//! cycle breaker see the import order instead and pick different edges
//! to reverse.
//!
//! Scope note: the graph-placer half (`combine`, `SimpleRowGraphPlacer`
//! row packing of multiple components into the parent's coordinate
//! space) is an E6 coordinate concern and is deferred. The >1-component
//! branch therefore asserts rather than half-running — every draw-uml
//! input and every current fixture is a single connected component, so
//! the reorder-in-place path is the only one exercised. When a
//! multi-component fixture lands, this is where `combine` + per-component
//! fresh `Random` streams get ported.

use super::graph::{LGraphArena, LGraphId, LNodeId};

/// Java `ComponentsProcessor.split(graph)`. Returns the component graphs
/// in creation order. In the single-component case (all current scope)
/// the input graph is returned with its `layerless_nodes` reordered into
/// DFS discovery order.
pub fn split(arena: &mut LGraphArena, graph: LGraphId) -> Vec<LGraphId> {
    // Java: `separate && (compatiblePortConstraints || !extPorts)`.
    let props = &arena.graphs[graph.0].props;
    let separate = props.separate_connected_components;
    let ext_ports = props.graph_properties.external_ports;
    // FIXED_ORDER / FIXED_RATIO / FIXED_POS are the incompatible ones.
    let compatible = !props.port_constraints.is_order_fixed();
    if !(separate && (compatible || !ext_ports)) {
        return vec![graph];
    }

    // `$dfs_1` reuses `LGraphElement.id` as the visited mark (0 = fresh).
    let nodes = arena.graphs[graph.0].layerless_nodes.clone();
    for &n in &nodes {
        arena.nodes[n.0].id = 0;
    }
    let mut components: Vec<Vec<LNodeId>> = Vec::new();
    for &n in &nodes {
        if arena.nodes[n.0].id == 0 {
            let mut component = Vec::new();
            dfs(arena, n, &mut component);
            components.push(component);
        }
    }

    assert!(
        components.len() <= 1,
        "multi-component graph separation (combine + per-component Random) \
         is deferred to E6; every ported-scope input is one connected component"
    );
    if let Some(component) = components.into_iter().next() {
        arena.graphs[graph.0].layerless_nodes = component;
    }
    vec![graph]
}

/// Java `ComponentsProcessor.dfs(node, data)`: collect `node`'s
/// connected component into `component` in DFS discovery order. Per
/// port ELK walks `getConnectedEdges()` — incoming edges first (yielding
/// the source port), then outgoing (yielding the target port) — and
/// recurses into the opposite port's owner.
fn dfs(arena: &mut LGraphArena, node: LNodeId, component: &mut Vec<LNodeId>) {
    if arena.nodes[node.0].id != 0 {
        return;
    }
    arena.nodes[node.0].id = 1;
    component.push(node);
    for port in arena.nodes[node.0].ports.clone() {
        for edge in arena.ports[port.0].incoming_edges.clone() {
            let source = arena.edges[edge.0].source.unwrap();
            let owner = arena.ports[source.0].owner.unwrap();
            dfs(arena, owner, component);
        }
        for edge in arena.ports[port.0].outgoing_edges.clone() {
            let target = arena.edges[edge.0].target.unwrap();
            let owner = arena.ports[target.0].owner.unwrap();
            dfs(arena, owner, component);
        }
    }
}
