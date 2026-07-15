//! Port of ELK `layered` phase 5 (`org.eclipse.elk.alg.layered.p5edges`):
//! edge routing. EPL-2.0.
//!
//! The `OrthogonalEdgeRouter` also assigns each layer its x-coordinate
//! (the layer axis, in the internal rightward orientation) as it walks
//! layer pairs: it places the left layer's nodes, then advances `xpos`
//! by the layer width plus the routing width derived from the slot count
//! the routing generator returns for that gap.
//!
//! This module holds the coordinate framing (`LayerSizeAndGraphHeight`
//! layer widths, `LGraphUtil.placeNodesHorizontally`, and the router
//! shell). The routing generator that produces the slot counts and bend
//! points is ported alongside.

pub mod orthogonal;

use super::graph::{LGraphArena, LGraphId, LNodeId};
use super::options::PortSide;

/// Java `LayerSizeAndGraphHeightCalculator` — the layer *width* part
/// (`layer.size.x` = widest node incl. left/right margins). Graph height
/// (the in-layer extent) is computed at export.
pub fn calculate_layer_sizes(arena: &mut LGraphArena, graph: LGraphId) {
    for li in 0..arena.graphs[graph.0].layers.len() {
        let mut size_x = 0.0f64;
        for &node in &arena.graphs[graph.0].layers[li].nodes {
            let n = &arena.nodes[node.0];
            size_x = size_x.max(n.size.x + n.margin.left + n.margin.right);
        }
        arena.graphs[graph.0].layers[li].size.x = size_x;
    }
}

/// Java `LGraphUtil.placeNodesHorizontally(layer, xoffset)`. With the
/// default (AUTOMATIC) alignment, a node's position within its layer is
/// biased by its port ratio `outports / (inports + outports)`.
pub fn place_nodes_horizontally(
    arena: &mut LGraphArena,
    graph: LGraphId,
    li: usize,
    xoffset: f64,
) {
    let nodes = arena.graphs[graph.0].layers[li].nodes.clone();
    let size_x = arena.graphs[graph.0].layers[li].size.x;
    let mut max_left = 0.0f64;
    let mut max_right = 0.0f64;
    for &node in &nodes {
        max_left = max_left.max(arena.nodes[node.0].margin.left);
        max_right = max_right.max(arena.nodes[node.0].margin.right);
    }
    for &node in &nodes {
        // AUTOMATIC alignment → port-count ratio.
        let mut inports = 0;
        let mut outports = 0;
        for &p in &arena.nodes[node.0].ports {
            if !arena.ports[p.0].incoming_edges.is_empty() {
                inports += 1;
            }
            if !arena.ports[p.0].outgoing_edges.is_empty() {
                outports += 1;
            }
        }
        let ratio =
            if inports + outports == 0 { 0.5 } else { outports as f64 / (inports + outports) as f64 };
        let node_size = arena.nodes[node.0].size.x;
        let mut xpos = (size_x - node_size) * ratio;
        if ratio > 0.5 {
            xpos -= max_right * 2.0 * (ratio - 0.5);
        } else if ratio < 0.5 {
            xpos += max_left * 2.0 * (0.5 - ratio);
        }
        let left_margin = arena.nodes[node.0].margin.left;
        if xpos < left_margin {
            xpos = left_margin;
        }
        let right_margin = arena.nodes[node.0].margin.right;
        if xpos > size_x - right_margin - node_size {
            xpos = size_x - right_margin - node_size;
        }
        arena.nodes[node.0].position.x = xoffset + xpos;
    }
}

/// Java `OrthogonalEdgeRouter.process` — the layer-x accumulation. Calls
/// `route` for each layer gap (`left`, `right`, `start_pos`) → slot
/// count; the routing generator fills bend points as a side effect.
/// Returns the graph width. No external ports occur in scope, so a null
/// layer is the only "external" case.
pub fn route_and_place(
    arena: &mut LGraphArena,
    graph: LGraphId,
    mut route: impl FnMut(&mut LGraphArena, Option<usize>, Option<usize>, f64) -> i32,
) -> f64 {
    calculate_layer_sizes(arena, graph);
    let sp = arena.graphs[graph.0].props.spacing;
    let (nn, ee, en) =
        (sp.node_node_between_layers, sp.edge_edge_between_layers, sp.edge_node_between_layers);
    let n = arena.graphs[graph.0].layers.len();

    let mut xpos = 0.0f64;
    let mut left: Option<usize> = None;
    // Iterate right = L0..L_{n-1}, then None (placing the last layer).
    let mut i = 0usize;
    loop {
        let right: Option<usize> = if i < n { Some(i) } else { None };
        if let Some(l) = left {
            place_nodes_horizontally(arena, graph, l, xpos);
            xpos += arena.graphs[graph.0].layers[l].size.x;
        }
        let start_pos = if left.is_none() { xpos } else { xpos + en };
        let slots = route(arena, left, right, start_pos);
        if std::env::var("ELK_DBG").is_ok() {
            let real = arena.graphs[graph.0]
                .layers
                .iter()
                .flat_map(|l| l.nodes.iter())
                .filter(|&&n| arena.nodes[n.0].node_type == super::graph::NodeType::Normal)
                .count();
            let left_len =
                left.map(|li| arena.graphs[graph.0].layers[li].nodes.len() as i64).unwrap_or(-1);
            eprintln!("SLOT real={real} left={left_len} slots={slots} xpos={xpos:.4}");
        }
        // Java: a layer is "external" when absent *or* when all its nodes
        // are east/west external-port dummies (compound boundary layers) —
        // those layers get no node-node spacing toward their neighbors.
        let all_external = |arena: &LGraphArena, li: usize| {
            arena.graphs[graph.0].layers[li].nodes.iter().all(|&n| {
                let normalized = match arena.nodes[n.0].props.ext_port_side {
                    PortSide::North => PortSide::West,
                    PortSide::South => PortSide::East,
                    s => s,
                };
                arena.nodes[n.0].node_type == super::graph::NodeType::ExternalPort
                    && matches!(normalized, PortSide::East | PortSide::West)
            })
        };
        let is_left_external = left.is_none_or(|li| all_external(arena, li));
        let is_right_external = right.is_none_or(|li| all_external(arena, li));
        if slots > 0 {
            let mut routing_width = (slots - 1) as f64 * ee;
            if left.is_some() {
                routing_width += en;
            }
            if right.is_some() {
                routing_width += en;
            }
            if routing_width < nn && !is_left_external && !is_right_external {
                routing_width = nn;
            }
            xpos += routing_width;
        } else if !is_left_external && !is_right_external {
            xpos += nn;
        }
        left = right;
        if right.is_none() {
            break;
        }
        i += 1;
    }
    arena.graphs[graph.0].size.x = xpos;
    xpos
}

/// Read-only view of a layer's nodes (for the routing generator).
pub fn layer_nodes(arena: &LGraphArena, graph: LGraphId, li: usize) -> Vec<LNodeId> {
    arena.graphs[graph.0].layers[li].nodes.clone()
}
