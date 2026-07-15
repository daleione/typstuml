//! Port of `org.eclipse.elk.alg.layered.p1cycles.GreedyCycleBreaker`
//! (Eades–Lin–Smyth greedy heuristic, phase 1 of layered).
//!
//! Assigns every node of one graph a linear "mark": sinks peel off the
//! right end, sources off the left end, and when only cycles remain
//! the node with maximum outflow−inflow moves left (ties broken with
//! the graph's `java.util.Random`, seed = `elk.randomSeed`). Every
//! edge pointing right-to-left in mark order is reversed in place
//! (`LEdge.reverse`); the REVERSED_EDGE_RESTORER intermediate
//! processor un-reverses them after routing.
//!
//! In hierarchical (INCLUDE_CHILDREN) runs this phase is not
//! hierarchy-aware: it runs once per graph in the nesting tree, each
//! with a fresh `Random(seed)` (`GraphConfigurator` seeds per graph).

use super::graph::{LGraphArena, LGraphId, LNodeId};
use super::random::JavaRandom;

/// Java `GreedyCycleBreaker.process(layeredGraph)`. Returns the edges
/// it reversed (callers/tests want them; Java flags the graph CYCLIC).
pub fn break_cycles(
    arena: &mut LGraphArena,
    graph: LGraphId,
    random: &mut JavaRandom,
) -> Vec<super::graph::LEdgeId> {
    let nodes: Vec<LNodeId> = arena.graphs[graph.0].layerless_nodes.clone();
    let mut unprocessed_node_count = nodes.len();
    let n = nodes.len();
    let mut indeg = vec![0i32; n];
    let mut outdeg = vec![0i32; n];
    let mut mark = vec![0i32; n];
    // Java reuses `LNode.id` as the index; the port keeps a local map
    // (arena ids are global, `mark` is per-graph).
    let index_of = |arena: &LGraphArena, node: LNodeId| -> usize {
        arena.nodes[node.0].id
    };
    for (index, &node) in nodes.iter().enumerate() {
        arena.nodes[node.0].id = index;
    }

    let mut sources: std::collections::VecDeque<LNodeId> = Default::default();
    let mut sinks: std::collections::VecDeque<LNodeId> = Default::default();

    let weight = |arena: &LGraphArena, edge: super::graph::LEdgeId| -> i32 {
        let priority = arena.edges[edge.0].props.priority;
        if priority > 0 { priority + 1 } else { 1 }
    };

    for (index, &node) in nodes.iter().enumerate() {
        for &port in &arena.nodes[node.0].ports {
            for &edge in &arena.ports[port.0].incoming_edges {
                if arena.edge_source_node(edge) == Some(node) {
                    continue; // self-loop
                }
                indeg[index] += weight(arena, edge);
            }
            for &edge in &arena.ports[port.0].outgoing_edges {
                if arena.edge_target_node(edge) == Some(node) {
                    continue; // self-loop
                }
                outdeg[index] += weight(arena, edge);
            }
        }
        if outdeg[index] == 0 {
            sinks.push_back(node);
        } else if indeg[index] == 0 {
            sources.push_back(node);
        }
    }

    let mut next_right = -1i32;
    let mut next_left = 1i32;

    // Java `updateNeighbors(node)`, shared by all three removal paths.
    fn update_neighbors(
        arena: &LGraphArena,
        node: LNodeId,
        mark: &[i32],
        indeg: &mut [i32],
        outdeg: &mut [i32],
        sources: &mut std::collections::VecDeque<LNodeId>,
        sinks: &mut std::collections::VecDeque<LNodeId>,
    ) {
        for &port in &arena.nodes[node.0].ports {
            // Java iterates getConnectedEdges(): incoming then
            // outgoing per port (see LPort's CombineIter).
            let edges: Vec<_> = arena.ports[port.0]
                .incoming_edges
                .iter()
                .chain(arena.ports[port.0].outgoing_edges.iter())
                .copied()
                .collect();
            for edge in edges {
                let (src, tgt) = (
                    arena.edges[edge.0].source.unwrap(),
                    arena.edges[edge.0].target.unwrap(),
                );
                let connected_port = if src == port { tgt } else { src };
                let endpoint = arena.ports[connected_port.0].owner.unwrap();
                if endpoint == node {
                    continue;
                }
                let mut priority = arena.edges[edge.0].props.priority;
                if priority < 0 {
                    priority = 0;
                }
                let index = arena.nodes[endpoint.0].id;
                if mark[index] == 0 {
                    if tgt == connected_port {
                        indeg[index] -= priority + 1;
                        if indeg[index] <= 0 && outdeg[index] > 0 {
                            sources.push_back(endpoint);
                        }
                    } else {
                        outdeg[index] -= priority + 1;
                        if outdeg[index] <= 0 && indeg[index] > 0 {
                            sinks.push_back(endpoint);
                        }
                    }
                }
            }
        }
    }

    let mut max_nodes: Vec<LNodeId> = Vec::new();
    while unprocessed_node_count > 0 {
        while let Some(sink) = sinks.pop_front() {
            mark[index_of(arena, sink)] = next_right;
            next_right -= 1;
            update_neighbors(arena, sink, &mark, &mut indeg, &mut outdeg, &mut sources, &mut sinks);
            unprocessed_node_count -= 1;
        }
        while let Some(source) = sources.pop_front() {
            mark[index_of(arena, source)] = next_left;
            next_left += 1;
            update_neighbors(
                arena, source, &mark, &mut indeg, &mut outdeg, &mut sources, &mut sinks,
            );
            unprocessed_node_count -= 1;
        }
        if unprocessed_node_count > 0 {
            let mut max_outflow = i32::MIN;
            max_nodes.clear();
            for &node in &nodes {
                let index = index_of(arena, node);
                if mark[index] == 0 {
                    let outflow = outdeg[index] - indeg[index];
                    if outflow >= max_outflow {
                        if outflow > max_outflow {
                            max_nodes.clear();
                            max_outflow = outflow;
                        }
                        max_nodes.push(node);
                    }
                }
            }
            assert!(max_outflow > i32::MIN);
            let max_node = max_nodes[random.next_int(max_nodes.len() as i32) as usize];
            mark[index_of(arena, max_node)] = next_left;
            next_left += 1;
            update_neighbors(
                arena, max_node, &mark, &mut indeg, &mut outdeg, &mut sources, &mut sinks,
            );
            unprocessed_node_count -= 1;
        }
    }

    // Shift negative marks above the positive range.
    let shift_base = (nodes.len() + 1) as i32;
    for m in mark.iter_mut() {
        if *m < 0 {
            *m += shift_base;
        }
    }

    // Reverse all edges that point right-to-left.
    let mut reversed = Vec::new();
    for &node in &nodes {
        let node_index = index_of(arena, node);
        for port in arena.nodes[node.0].ports.clone() {
            for edge in arena.ports[port.0].outgoing_edges.clone() {
                let target_node = arena.edge_target_node(edge).unwrap();
                let target_ix = index_of(arena, target_node);
                if mark[node_index] > mark[target_ix] {
                    arena.edge_reverse(edge, true);
                    reversed.push(edge);
                }
            }
        }
    }
    reversed
}
