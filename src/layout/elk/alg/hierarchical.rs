//! The compound (INCLUDE_CHILDREN) layout driver and the hierarchical
//! intermediate processors it needs beyond the flat pipeline:
//! `LayerSizeAndGraphHeightCalculator`, the east/west subset of
//! `HierarchicalPortOrthogonalEdgeRouter`, and
//! `HierarchicalNodeResizingProcessor` (with `LGraphUtil`'s
//! `getExternalPortPosition`/`resizeNode`). EPL-2.0 (see `LICENSE.md`).
//!
//! Control flow mirrors `ElkLayered.hierarchicalLayout`: graphs are
//! collected bottom-up; every graph runs its processors until the
//! hierarchy-aware crossing minimizer, which executes once on the root
//! (diving into swept-into children); then every graph — children first —
//! runs to completion (P4, P5, after-phase-5 chain ending in the node
//! resizer, which feeds the parent node's size and port positions).
//! Finally `CompoundGraphPostprocessor` reassembles the cross-hierarchy
//! edges on the root.
//!
//! **Frames**: the algorithm runs in the normalized rightward frame
//! (position.x = layer axis) while this port's graph *paddings* stay in
//! the user frame (ELK rotates them at import; we skip that rotation).
//! [`internal_padding`] transposes on the fly; every internal-frame
//! computation in this module uses it. External-port sides are matched
//! via `rightward` (NORTH→WEST, SOUTH→EAST) like the rest of phase 3+.

use std::collections::HashMap;

use super::graph::{LGraphArena, LGraphId, LNodeId, NodeType};
use super::math::{Insets, KVector};
use super::options::{PortConstraints, PortSide};
use super::random::JavaRandom;
use super::{
    compound, high_degree, intermediate, layer_constraint, p1_cycles, p2_layers, p4nodes,
    preserve_order,
};
use super::p3order::layer_sweep;
use super::p5edges::orthogonal;

/// Normalize a side into the rightward frame (NORTH→WEST, SOUTH→EAST).
fn rightward(side: PortSide) -> PortSide {
    match side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    }
}

/// The graph's padding in the internal rightward frame. ELK's
/// `TO_INTERNAL_LTR` transpose maps the user's DOWN-frame insets to
/// left↔top / right↔bottom; this port skips the rotation and stores user
/// paddings, so internal-frame code transposes here instead.
pub fn internal_padding(arena: &LGraphArena, graph: LGraphId) -> Insets {
    let p = arena.graphs[graph.0].padding;
    Insets { left: p.top, right: p.bottom, top: p.left, bottom: p.right }
}

// ----------------------------------------------------------------------
// LayerSizeAndGraphHeightCalculator (before phase 5)
// ----------------------------------------------------------------------

/// Port of `LayerSizeAndGraphHeightCalculator`: per-layer sizes, the
/// graph's height (in-layer extent) and the offset shift that puts the
/// topmost content at 0. `SPACING_PORTS_SURROUNDING` is never set in
/// scope (defaults to 0), so the external-dummy padding terms vanish.
pub fn layer_size_and_graph_height_calculator(arena: &mut LGraphArena, graph: LGraphId) {
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut found_nodes = false;

    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        arena.graphs[graph.0].layers[li].size = KVector::default();
        let nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        if nodes.is_empty() {
            continue;
        }
        found_nodes = true;

        let mut width = 0.0f64;
        for &n in &nodes {
            let node = &arena.nodes[n.0];
            width = width.max(node.size.x + node.margin.left + node.margin.right);
        }
        arena.graphs[graph.0].layers[li].size.x = width;

        let first = &arena.nodes[nodes[0].0];
        let top = first.position.y - first.margin.top;
        let last = &arena.nodes[nodes[nodes.len() - 1].0];
        let bottom = last.position.y + last.size.y + last.margin.bottom;
        arena.graphs[graph.0].layers[li].size.y = bottom - top;

        min_y = min_y.min(top);
        max_y = max_y.max(bottom);
    }

    if !found_nodes {
        min_y = 0.0;
        max_y = 0.0;
    }

    arena.graphs[graph.0].size.y = max_y - min_y;
    arena.graphs[graph.0].offset.y -= min_y;
}

// ----------------------------------------------------------------------
// HierarchicalPortOrthogonalEdgeRouter (after phase 5) — east/west subset
// ----------------------------------------------------------------------

/// Port of `HierarchicalPortOrthogonalEdgeRouter` for graphs whose
/// external ports are all (normalized) east/west: steps 1–4 handle
/// north/south hierarchical ports and are unreachable; step 5 fixes the
/// dummies' layer-axis coordinates; step 6 straightens edge segments the
/// move may have slanted. The y branch of step 5 only fires for
/// FIXED_RATIO / FIXED_POS graph port constraints — never set in scope.
pub fn hierarchical_port_orthogonal_edge_router(arena: &mut LGraphArena, graph: LGraphId) {
    if arena.graphs[graph.0].layers.is_empty() {
        return;
    }
    fix_coordinates(arena, graph, 0);
    let last = arena.graphs[graph.0].layers.len() - 1;
    fix_coordinates(arena, graph, last);
    correct_slanted_edge_segments(arena, graph, 0);
    correct_slanted_edge_segments(arena, graph, last);
}

/// Java `fixCoordinates(layer, constraints, graph)`, east/west branches.
fn fix_coordinates(arena: &mut LGraphArena, graph: LGraphId, layer_index: usize) {
    let pad = internal_padding(arena, graph);
    let offset = arena.graphs[graph.0].offset;
    let size = arena.graphs[graph.0].size;
    for n in arena.graphs[graph.0].layers[layer_index].nodes.clone() {
        if arena.nodes[n.0].node_type != NodeType::ExternalPort {
            continue;
        }
        let ext_side = rightward(arena.nodes[n.0].props.ext_port_side);
        match ext_side {
            PortSide::East => {
                arena.nodes[n.0].position.x = size.x + pad.right - offset.x;
            }
            PortSide::West => {
                arena.nodes[n.0].position.x = -offset.x - pad.left;
            }
            _ => panic!("north/south external ports are outside the ported scope"),
        }
        // y: only FIXED_RATIO / FIXED_POS graph port constraints move it —
        // absent in scope, the placement's y stands.
    }
}

/// Java `correctSlantedEdgeSegments(layer)`: after moving a dummy along
/// the layer axis, the first/last bend of each incident edge must stay
/// aligned with the dummy's anchor on the in-layer axis.
fn correct_slanted_edge_segments(arena: &mut LGraphArena, graph: LGraphId, layer_index: usize) {
    for n in arena.graphs[graph.0].layers[layer_index].nodes.clone() {
        if arena.nodes[n.0].node_type != NodeType::ExternalPort {
            continue;
        }
        let ext_side = rightward(arena.nodes[n.0].props.ext_port_side);
        if !matches!(ext_side, PortSide::East | PortSide::West) {
            continue;
        }
        for port in arena.nodes[n.0].ports.clone() {
            let edges: Vec<_> = arena.ports[port.0]
                .incoming_edges
                .iter()
                .chain(arena.ports[port.0].outgoing_edges.iter())
                .copied()
                .collect();
            for edge in edges {
                if arena.edges[edge.0].bend_points.is_empty() {
                    continue;
                }
                let source = arena.edges[edge.0].source.unwrap();
                if arena.ports[source.0].owner == Some(n) {
                    let anchor_y = absolute_anchor(arena, source).y;
                    arena.edges[edge.0].bend_points.first_mut().unwrap().y = anchor_y;
                }
                let target = arena.edges[edge.0].target.unwrap();
                if arena.ports[target.0].owner == Some(n) {
                    let anchor_y = absolute_anchor(arena, target).y;
                    arena.edges[edge.0].bend_points.last_mut().unwrap().y = anchor_y;
                }
            }
        }
    }
}

/// Java `LPort.getAbsoluteAnchor()`: node position + port position +
/// port anchor (graph-local).
pub fn absolute_anchor(arena: &LGraphArena, port: super::graph::LPortId) -> KVector {
    let node = arena.ports[port.0].owner.unwrap();
    KVector::new(
        arena.nodes[node.0].position.x + arena.ports[port.0].position.x + arena.ports[port.0].anchor.x,
        arena.nodes[node.0].position.y + arena.ports[port.0].position.y + arena.ports[port.0].anchor.y,
    )
}

/// Java `LGraphUtil.changeCoordSystem(point, oldGraph, newGraph)`: the
/// vector to add to an `old`-frame point to express it in the `new`
/// frame. Walks both graphs to the root, adding each level's offset,
/// internal-frame padding and parent-node position.
pub fn change_coord_system(
    arena: &LGraphArena,
    old: LGraphId,
    new: LGraphId,
) -> KVector {
    if old == new {
        return KVector::default();
    }
    let up = |mut g: LGraphId| {
        let mut v = KVector::default();
        loop {
            v.x += arena.graphs[g.0].offset.x;
            v.y += arena.graphs[g.0].offset.y;
            let Some(parent) = arena.graphs[g.0].parent_node else { break };
            let pad = internal_padding(arena, g);
            v.x += pad.left + arena.nodes[parent.0].position.x;
            v.y += pad.top + arena.nodes[parent.0].position.y;
            g = arena.nodes[parent.0].graph;
        }
        v
    };
    let a = up(old);
    let b = up(new);
    KVector::new(a.x - b.x, a.y - b.y)
}

/// Whether `node` lies inside the nested-graph subtree of `ancestor`
/// (Java `LGraphUtil.isDescendant`).
pub fn is_descendant(arena: &LGraphArena, node: LNodeId, ancestor: LNodeId) -> bool {
    let mut g = arena.nodes[node.0].graph;
    loop {
        let Some(parent) = arena.graphs[g.0].parent_node else { return false };
        if parent == ancestor {
            return true;
        }
        g = arena.nodes[parent.0].graph;
    }
}

// ----------------------------------------------------------------------
// HierarchicalNodeResizingProcessor (after phase 5, last)
// ----------------------------------------------------------------------

/// Port of `HierarchicalNodeResizingProcessor`: nodes return to
/// `layerless_nodes` (layers are cleared), the graph is resized
/// (identity in scope — no MINIMUM_SIZE constraint or content
/// alignment), and for nested graphs the layout is transferred to the
/// parent node: external ports get their real positions, the parent
/// becomes FIXED_POS and its size the child's actual size.
pub fn hierarchical_node_resizing_processor(arena: &mut LGraphArena, graph: LGraphId) {
    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        let nodes = std::mem::take(&mut arena.graphs[graph.0].layers[li].nodes);
        for &n in &nodes {
            arena.nodes[n.0].layer = None;
        }
        arena.graphs[graph.0].layerless_nodes.extend(nodes);
    }
    arena.graphs[graph.0].layers.clear();

    resize_graph(arena, graph);
    if arena.graphs[graph.0].parent_node.is_some() {
        graph_layout_to_node(arena, graph);
    }
}

/// Java `resizeGraph` + `resizeGraphNoReallyIMeanIt`: without
/// MINIMUM_SIZE constraints and content alignment (absent in scope) it
/// reduces to `size = actualSize - padding` — an identity that keeps the
/// bookkeeping shape of the original.
fn resize_graph(arena: &mut LGraphArena, graph: LGraphId) {
    let pad = internal_padding(arena, graph);
    let calculated = KVector::new(
        arena.graphs[graph.0].size.x + pad.left + pad.right,
        arena.graphs[graph.0].size.y + pad.top + pad.bottom,
    );
    let new_size = calculated;
    arena.graphs[graph.0].size.x = new_size.x - pad.left - pad.right;
    arena.graphs[graph.0].size.y = new_size.y - pad.top - pad.bottom;
}

/// The graph's actual size (`LGraph.getActualSize`) in the internal
/// frame: content size plus internal-frame padding.
pub fn actual_size(arena: &LGraphArena, graph: LGraphId) -> KVector {
    let pad = internal_padding(arena, graph);
    KVector::new(
        arena.graphs[graph.0].size.x + pad.left + pad.right,
        arena.graphs[graph.0].size.y + pad.top + pad.bottom,
    )
}

/// Java `graphLayoutToNode`: transfer a finished child layout to its
/// parent node — external ports get positions via
/// `getExternalPortPosition`, the parent's port constraints become
/// FIXED_POS, and its size the child graph's actual size.
fn graph_layout_to_node(arena: &mut LGraphArena, graph: LGraphId) {
    let parent = arena.graphs[graph.0].parent_node.unwrap();

    for n in arena.graphs[graph.0].layerless_nodes.clone() {
        if arena.nodes[n.0].node_type != NodeType::ExternalPort {
            continue;
        }
        let Some(origin_port) = arena.nodes[n.0].props.origin_port else { continue };
        let port_size = arena.ports[origin_port.0].size;
        let pos = get_external_port_position(arena, graph, n, port_size.x, port_size.y);
        arena.ports[origin_port.0].position = pos;
        // Java also re-stamps the port's side from EXT_PORT_SIDE — ours
        // never changed (the un-rotated original), so it is a no-op here.
    }

    let new_size = actual_size(arena, graph);
    if arena.graphs[graph.0].props.graph_properties.external_ports {
        arena.nodes[parent.0].props.port_constraints = PortConstraints::FixedPos;
        // (Java also adds NON_FREE_PORTS to the parent graph's properties;
        // nothing in the remaining pipeline reads it in scope.)
    }
    // LGraphUtil.resizeNode(node, size, movePorts=false, moveLabels=true):
    // no labels in scope — just the size.
    arena.nodes[parent.0].size = new_size;
}

/// Java `LGraphUtil.getExternalPortPosition` (east/west branches via the
/// normalized side): the *returned* vector is the parent-node port
/// position (relative to the parent's border box, internal frame); the
/// dummy's own position is fixed up as a side effect, exactly like the
/// original.
fn get_external_port_position(
    arena: &mut LGraphArena,
    graph: LGraphId,
    dummy: LNodeId,
    port_width: f64,
    port_height: f64,
) -> KVector {
    let mut port_position = arena.nodes[dummy.0].position;
    port_position.x += arena.nodes[dummy.0].size.x / 2.0;
    port_position.y += arena.nodes[dummy.0].size.y / 2.0;
    let port_offset = arena.nodes[dummy.0].props.port_border_offset;

    let graph_size = arena.graphs[graph.0].size;
    let pad = internal_padding(arena, graph);
    let graph_offset = arena.graphs[graph.0].offset;

    match rightward(arena.nodes[dummy.0].props.ext_port_side) {
        PortSide::East => {
            port_position.x = graph_size.x + pad.left + pad.right + port_offset;
            port_position.y += pad.top + graph_offset.y - port_height / 2.0;
            arena.nodes[dummy.0].position.x = graph_size.x + pad.right + port_offset - graph_offset.x;
        }
        PortSide::West => {
            port_position.x = -port_width - port_offset;
            port_position.y += pad.top + graph_offset.y - port_height / 2.0;
            arena.nodes[dummy.0].position.x = -(pad.left + port_offset + graph_offset.x);
        }
        _ => panic!("north/south external ports are outside the ported scope"),
    }
    port_position
}

// ----------------------------------------------------------------------
// Driver
// ----------------------------------------------------------------------

/// Everything the exporter needs from a compound layout run beyond the
/// arena itself.
pub struct CompoundLayoutResult {
    /// Original cross-hierarchy edge → the graph whose coordinate frame
    /// its restored bend points live in (the postprocessor's reference
    /// graph).
    pub reference_graphs: HashMap<super::graph::LEdgeId, LGraphId>,
}

/// Run the full compound pipeline on an imported graph: compound
/// preprocessing, per-graph preparation up to phase 3, the
/// hierarchy-aware crossing minimizer on the root, per-graph P4/P5 and
/// the after-phase-5 chain (bottom-up, children before parents), and
/// compound postprocessing.
pub fn layout_compound(arena: &mut LGraphArena, top: LGraphId) -> CompoundLayoutResult {
    let cross_map = compound::preprocess(arena, top);

    // collectAllGraphsBottomUp
    let mut graphs: Vec<LGraphId> = Vec::new();
    let mut discover = vec![top];
    while let Some(g) = discover.pop() {
        graphs.insert(0, g);
        for &n in &arena.graphs[g.0].layerless_nodes {
            if let Some(ng) = arena.nodes[n.0].nested_graph {
                discover.push(ng);
            }
        }
    }

    // Per graph up to phase 3, each on its own fresh Random(1) (ELK's
    // GraphConfigurator seeds one per graph).
    let mut root_random = JavaRandom::new(1);
    let mut child_randoms: HashMap<LGraphId, JavaRandom> = HashMap::new();
    for &g in &graphs {
        let mut random = JavaRandom::new(1);
        p1_cycles::break_cycles(arena, g, &mut random);
        let hidden = layer_constraint::preprocess(arena, g);
        p2_layers::layer_nodes(arena, g);
        if arena.graphs[g.0].props.high_degree_nodes_treatment {
            high_degree::process(arena, g);
        }
        layer_constraint::postprocess(arena, g, &hidden);
        intermediate::long_edge_splitter(arena, g);
        intermediate::port_side_processor(arena, g);
        intermediate::inverted_port_processor(arena, g);
        intermediate::port_list_sorter(arena, g);
        preserve_order::sort_by_input_model(arena, g);
        if g == top {
            root_random = random;
        } else {
            child_randoms.insert(g, random);
        }
    }

    // Phase 3 (hierarchy-aware, runs once on the root).
    layer_sweep::layer_sweep_crossing_minimizer(arena, top, &mut root_random, &mut child_randoms);

    // Per graph to completion, children before parents so a parent's P4
    // sees the group-node sizes its children just computed.
    for &g in &graphs {
        p4nodes::prepare_placement(arena, g);
        p4nodes::bk::place(arena, g);
        layer_size_and_graph_height_calculator(arena, g);
        {
            let random =
                if g == top { &mut root_random } else { child_randoms.get_mut(&g).unwrap() };
            let width = orthogonal::route_orthogonal(arena, g, random);
            arena.graphs[g.0].size.x = width;
        }
        if arena.graphs[g.0].props.graph_properties.external_ports {
            hierarchical_port_orthogonal_edge_router(arena, g);
        }
        intermediate::long_edge_joiner(arena, g);
        // HORIZONTAL_COMPACTOR sits between the joiner and the reversed-
        // edge restorer in ELK's after-phase-5 order.
        if arena.graphs[g.0].props.post_compaction_left {
            super::compaction::horizontal_graph_compactor_left(arena, g);
        }
        intermediate::reversed_edge_restorer(arena, g);
        hierarchical_node_resizing_processor(arena, g);
    }

    let reference_graphs = compound::postprocess(arena, top, &cross_map);
    CompoundLayoutResult { reference_graphs }
}
