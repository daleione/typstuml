//! Ports of the ELK `layered` **intermediate** processors that run
//! between phases (EPL-2.0, see `LICENSE.md`). Upstream:
//! `org.eclipse.elk.alg.layered.intermediate`.
//!
//! This file grows one processor at a time as the phase pipeline is
//! filled in.

use std::collections::VecDeque;

use super::graph::{LEdgeId, LGraphArena, LGraphId, LLabelId, LNodeId, LPortId, NodeType};
use super::math::KVector;
use super::options::{
    Direction, EdgeLabelPlacement, LabelSide, PortConstraints, PortSide,
};

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
    // Java `moveHeadLabels(edge, dummyEdge)`: only HEAD-placed labels
    // move — those are asserted out of the ported scope at import, and
    // CENTER labels have been moved onto label dummies before any split
    // reaches here, so this is a structural no-op.
    debug_assert!(arena.edges[edge.0]
        .labels
        .iter()
        .all(|&l| arena.labels[l.0].props.placement != EdgeLabelPlacement::Head));
    dummy_edge
}

/// Java `LongEdgeSplitter.setDummyNodeProperties`: carry the original
/// edge's real source/target ports through the dummy chain, marking the
/// chain when it runs through a `LABEL` dummy.
fn set_dummy_node_properties(
    arena: &mut LGraphArena,
    dummy_node: LNodeId,
    in_edge: LEdgeId,
    out_edge: LEdgeId,
) {
    let in_source_node = arena.edge_source_node(in_edge).unwrap();
    let out_target_node = arena.edge_target_node(out_edge).unwrap();

    if arena.nodes[in_source_node.0].node_type == NodeType::LongEdge {
        arena.nodes[dummy_node.0].props.long_edge_source =
            arena.nodes[in_source_node.0].props.long_edge_source;
        arena.nodes[dummy_node.0].props.long_edge_target =
            arena.nodes[in_source_node.0].props.long_edge_target;
        arena.nodes[dummy_node.0].props.long_edge_has_label_dummies =
            arena.nodes[in_source_node.0].props.long_edge_has_label_dummies;
    } else if arena.nodes[in_source_node.0].node_type == NodeType::Label {
        arena.nodes[dummy_node.0].props.long_edge_source =
            arena.nodes[in_source_node.0].props.long_edge_source;
        arena.nodes[dummy_node.0].props.long_edge_target =
            arena.nodes[in_source_node.0].props.long_edge_target;
        arena.nodes[dummy_node.0].props.long_edge_has_label_dummies = true;
    } else if arena.nodes[out_target_node.0].node_type == NodeType::Label {
        arena.nodes[dummy_node.0].props.long_edge_source =
            arena.nodes[out_target_node.0].props.long_edge_source;
        arena.nodes[dummy_node.0].props.long_edge_target =
            arena.nodes[out_target_node.0].props.long_edge_target;
        arena.nodes[dummy_node.0].props.long_edge_has_label_dummies = true;
    } else {
        arena.nodes[dummy_node.0].props.long_edge_source = arena.edges[in_edge.0].source;
        arena.nodes[dummy_node.0].props.long_edge_target = arena.edges[out_edge.0].target;
    }
}

// ----------------------------------------------------------------------
// LabelDummyInserter (before phase 2)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.LabelDummyInserter`.
/// Replaces every non-self-loop edge carrying CENTER labels by a `LABEL`
/// dummy node (via [`split_edge`]) that reserves the labels' space; the
/// labels move from the edge to the dummy's `REPRESENTED_LABELS`.
///
/// Frames: ELK runs after its `TO_INTERNAL_LTR` rotation, where a
/// vertical layout's label extents are transposed. This port stores label
/// sizes in the user frame, so on the vertical branch the roles of
/// `size.x`/`size.y` swap relative to the Java source.
pub fn label_dummy_inserter(arena: &mut LGraphArena, graph: LGraphId) {
    let edge_label_spacing = arena.graphs[graph.0].props.spacing.edge_label;
    let label_label_spacing = arena.graphs[graph.0].props.spacing.label_label;
    let vertical = matches!(
        arena.graphs[graph.0].props.direction,
        Direction::Up | Direction::Down
    );

    let nodes = arena.graphs[graph.0].layerless_nodes.clone();
    for node in nodes {
        for edge in arena.node_outgoing_edges(node) {
            if !edge_needs_to_be_processed(arena, edge) {
                continue;
            }
            // EDGE_THICKNESS defaults to 1 and is never overridden in scope.
            let thickness = 1.0f64;
            let dummy_node = create_label_dummy(arena, graph, edge, thickness);

            let mut dummy_size = arena.nodes[dummy_node.0].size;
            let mut represented: Vec<LLabelId> = Vec::new();
            let mut remaining: Vec<LLabelId> = Vec::new();
            for label in arena.edges[edge.0].labels.clone() {
                if arena.labels[label.0].props.placement == EdgeLabelPlacement::Center {
                    let user = arena.labels[label.0].size;
                    if vertical {
                        // Java internal sizes are transposed: internal x =
                        // user height, internal y = user width.
                        dummy_size.x += user.y + label_label_spacing;
                        dummy_size.y = dummy_size.y.max(user.x);
                    } else {
                        dummy_size.x = dummy_size.x.max(user.x);
                        dummy_size.y += user.y + label_label_spacing;
                    }
                    represented.push(label);
                } else {
                    remaining.push(label);
                }
            }
            // Remove the superfluous label-label spacing; add the
            // edge-label spacing and edge thickness.
            if vertical {
                dummy_size.x -= label_label_spacing;
                dummy_size.y += edge_label_spacing + thickness;
            } else {
                dummy_size.y += edge_label_spacing - label_label_spacing + thickness;
            }
            arena.nodes[dummy_node.0].size = dummy_size;
            arena.nodes[dummy_node.0].props.represented_labels = represented;
            arena.edges[edge.0].labels = remaining;
        }
    }
    // Java collects the dummies in `newDummyNodes` and appends them after
    // the loop; `new_node` appends at creation, which yields the same
    // final layerless order (the loop iterates a snapshot).
}

/// Java `LabelDummyInserter.edgeNeedsToBeProcessed`: not a self-loop and
/// carries at least one CENTER label.
fn edge_needs_to_be_processed(arena: &LGraphArena, edge: LEdgeId) -> bool {
    arena.edge_source_node(edge) != arena.edge_target_node(edge)
        && arena.edges[edge.0]
            .labels
            .iter()
            .any(|&l| arena.labels[l.0].props.placement == EdgeLabelPlacement::Center)
}

/// Java `LabelDummyInserter.createLabelDummy`: the `LABEL` dummy with
/// FIXED_POS ports, `ORIGIN` = the split edge, `LONG_EDGE_SOURCE/TARGET`
/// captured *before* the split reroutes the edge.
fn create_label_dummy(
    arena: &mut LGraphArena,
    graph: LGraphId,
    edge: LEdgeId,
    thickness: f64,
) -> LNodeId {
    let dummy = arena.new_node(graph);
    arena.nodes[dummy.0].node_type = NodeType::Label;
    arena.nodes[dummy.0].props.origin_edge = Some(edge);
    arena.nodes[dummy.0].props.port_constraints = PortConstraints::FixedPos;
    arena.nodes[dummy.0].props.long_edge_source = arena.edges[edge.0].source;
    arena.nodes[dummy.0].props.long_edge_target = arena.edges[edge.0].target;

    split_edge(arena, edge, dummy);

    // Place ports at the edge's center: floor(thickness / 2).
    let port_pos = (thickness / 2.0).floor();
    for port in arena.nodes[dummy.0].ports.clone() {
        arena.ports[port.0].position.y = port_pos;
    }
    dummy
}

// ----------------------------------------------------------------------
// LabelDummySwitcher (before phase 4)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.LabelDummySwitcher`
/// for the default `MEDIAN_LAYER` center-label placement strategy (the
/// only one reachable in scope: draw-uml never sets the option, and
/// per-label overrides are not imported). Each label dummy swaps places
/// with the long-edge dummy in its edge's median layer, preserving both
/// in-layer positions; long-edge dummies left of the label dummy are
/// flagged `LONG_EDGE_BEFORE_LABEL_DUMMY`.
pub fn label_dummy_switcher(arena: &mut LGraphArena, graph: LGraphId) {
    // assignIdsToLayers: ids equal indices here, so layer indices serve
    // directly. Gather the infos first (a swap for one long edge never
    // touches another's dummies — mergeEdges is false in scope).
    struct LabelDummyInfo {
        label_dummy: LNodeId,
        left: Vec<LNodeId>,
        right: Vec<LNodeId>,
        leftmost_layer: usize,
        rightmost_layer: usize,
    }

    let mut infos: Vec<LabelDummyInfo> = Vec::new();
    for layer in &arena.graphs[graph.0].layers {
        for &node in &layer.nodes {
            if arena.nodes[node.0].node_type != NodeType::Label {
                continue;
            }
            // Long-edge dummies to the left (walked backwards, reversed).
            let mut left = Vec::new();
            let mut source = node;
            loop {
                source = arena
                    .edge_source_node(arena.node_incoming_edges(source)[0])
                    .unwrap();
                if arena.nodes[source.0].node_type == NodeType::LongEdge {
                    left.push(source);
                } else {
                    break;
                }
            }
            left.reverse();
            // Long-edge dummies to the right.
            let mut right = Vec::new();
            let mut target = node;
            loop {
                target = arena
                    .edge_target_node(arena.node_outgoing_edges(target)[0])
                    .unwrap();
                if arena.nodes[target.0].node_type == NodeType::LongEdge {
                    right.push(target);
                } else {
                    break;
                }
            }
            let leftmost_layer = left
                .first()
                .map_or(arena.nodes[node.0].layer.unwrap(), |&d| arena.nodes[d.0].layer.unwrap());
            let rightmost_layer = right
                .last()
                .map_or(arena.nodes[node.0].layer.unwrap(), |&d| arena.nodes[d.0].layer.unwrap());
            infos.push(LabelDummyInfo {
                label_dummy: node,
                left,
                right,
                leftmost_layer,
                rightmost_layer,
            });
        }
    }

    for info in &infos {
        // findMedianLayerTargetId: lower median of the spanned layers.
        let layers = info.rightmost_layer - info.leftmost_layer + 1;
        let lower_median = (layers - 1) / 2;
        let target_layer = info.leftmost_layer + lower_median;

        // assignLayer: swap only when not already there. (layerWidths
        // bookkeeping is dead for MEDIAN_LAYER: width-based strategies
        // never run, and findMaxNonDummyNodeWidth returns 0 for vertical
        // layouts anyway.)
        if target_layer != info.leftmost_layer + info.left.len() {
            let i = target_layer - info.leftmost_layer;
            let other = if i < info.left.len() {
                info.left[i]
            } else if i == info.left.len() {
                info.label_dummy
            } else {
                info.right[i - info.left.len() - 1]
            };
            swap_nodes(arena, graph, info.label_dummy, other);
        }

        // updateLongEdgeSourceLabelDummyInfo: flag predecessors.
        let mut pred = arena
            .edge_source_node(arena.node_incoming_edges(info.label_dummy)[0])
            .unwrap();
        while arena.nodes[pred.0].node_type == NodeType::LongEdge {
            arena.nodes[pred.0].props.long_edge_before_label_dummy = true;
            pred = arena
                .edge_source_node(arena.node_incoming_edges(pred)[0])
                .unwrap();
        }
    }
}

/// Java `LabelDummySwitcher.swapNodes`: exchange the two dummies' layers
/// (keeping each in-layer position) and reroute their edges onto each
/// other's ports.
fn swap_nodes(arena: &mut LGraphArena, graph: LGraphId, dummy1: LNodeId, dummy2: LNodeId) {
    let layer1 = arena.nodes[dummy1.0].layer.unwrap();
    let layer2 = arena.nodes[dummy2.0].layer.unwrap();
    let pos1 = arena.graphs[graph.0].layers[layer1]
        .nodes
        .iter()
        .position(|&n| n == dummy1)
        .unwrap();
    let pos2 = arena.graphs[graph.0].layers[layer2]
        .nodes
        .iter()
        .position(|&n| n == dummy2)
        .unwrap();

    let west_port = |arena: &LGraphArena, n: LNodeId| {
        arena.nodes[n.0]
            .ports
            .iter()
            .copied()
            .find(|&p| arena.ports[p.0].side == PortSide::West)
            .expect("dummy has a WEST input port")
    };
    let east_port = |arena: &LGraphArena, n: LNodeId| {
        arena.nodes[n.0]
            .ports
            .iter()
            .copied()
            .find(|&p| arena.ports[p.0].side == PortSide::East)
            .expect("dummy has an EAST output port")
    };
    let input1 = west_port(arena, dummy1);
    let output1 = east_port(arena, dummy1);
    let input2 = west_port(arena, dummy2);
    let output2 = east_port(arena, dummy2);

    let incoming1 = arena.ports[input1.0].incoming_edges.clone();
    let outgoing1 = arena.ports[output1.0].outgoing_edges.clone();
    let incoming2 = arena.ports[input2.0].incoming_edges.clone();
    let outgoing2 = arena.ports[output2.0].outgoing_edges.clone();

    arena.node_set_layer_at_index(graph, dummy1, layer2, pos2);
    for edge in incoming2 {
        arena.edge_set_target(edge, Some(input1));
    }
    for edge in outgoing2 {
        arena.edge_set_source(edge, Some(output1));
    }

    arena.node_set_layer_at_index(graph, dummy2, layer1, pos1);
    for edge in incoming1 {
        arena.edge_set_target(edge, Some(input2));
    }
    for edge in outgoing1 {
        arena.edge_set_source(edge, Some(output2));
    }
}

// ----------------------------------------------------------------------
// LabelSideSelector (before phase 4, after LabelDummySwitcher)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.LabelSideSelector`
/// for the SMART strategies. `edgeLabels.sideSelection` defaults to
/// `SMART_DOWN`; ELK's `TO_INTERNAL_LTR` transpose turns it into
/// `SMART_UP` for vertical layouts — this port skips the rotation, so the
/// transpose is applied here (vertical → default side ABOVE). End-label
/// side selection (`smartForRegularNode`) is a structural no-op in scope
/// (no head/tail labels exist) and is omitted.
pub fn label_side_selector(arena: &mut LGraphArena, graph: LGraphId) {
    let vertical = matches!(
        arena.graphs[graph.0].props.direction,
        Direction::Up | Direction::Down
    );
    let default_side = if vertical { LabelSide::Above } else { LabelSide::Below };

    let mut dummy_queue: VecDeque<LNodeId> = VecDeque::new();
    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        let mut top_group = true;
        let mut label_dummies_in_queue = 0usize;
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            match arena.nodes[node.0].node_type {
                NodeType::Label => {
                    label_dummies_in_queue += 1;
                    dummy_queue.push_back(node);
                }
                NodeType::LongEdge => {
                    dummy_queue.push_back(node);
                }
                _ => {
                    // (NORMAL nodes additionally run end-label side
                    // selection in Java — no end labels in scope.)
                    if !dummy_queue.is_empty() {
                        smart_for_consecutive_dummy_run(
                            arena,
                            &mut dummy_queue,
                            label_dummies_in_queue,
                            top_group,
                            false,
                            default_side,
                        );
                    }
                    top_group = false;
                    label_dummies_in_queue = 0;
                }
            }
        }
        if !dummy_queue.is_empty() {
            smart_for_consecutive_dummy_run(
                arena,
                &mut dummy_queue,
                label_dummies_in_queue,
                top_group,
                true,
                default_side,
            );
        }
    }
}

/// Java `LNode.isInlineEdgeLabel`.
fn is_inline_edge_label(arena: &LGraphArena, node: LNodeId) -> bool {
    arena.nodes[node.0].node_type == NodeType::Label
        && arena.nodes[node.0]
            .props
            .represented_labels
            .iter()
            .all(|&l| arena.labels[l.0].props.inline)
}

/// Java `LabelSideSelector.smartForConsecutiveDummyNodeRun`.
fn smart_for_consecutive_dummy_run(
    arena: &mut LGraphArena,
    dummy_nodes: &mut VecDeque<LNodeId>,
    label_dummy_count: usize,
    top_group: bool,
    bottom_group: bool,
    default_side: LabelSide,
) {
    debug_assert!(!dummy_nodes.is_empty());
    let front = *dummy_nodes.front().unwrap();
    let back = *dummy_nodes.back().unwrap();

    if top_group
        && (!bottom_group || dummy_nodes.len() > 1)
        && label_dummy_count == 1
        && arena.nodes[front.0].node_type == NodeType::Label
    {
        apply_label_side(arena, front, LabelSide::Above);
    } else if bottom_group
        && (!top_group || dummy_nodes.len() > 1)
        && label_dummy_count == 1
        && arena.nodes[back.0].node_type == NodeType::Label
    {
        apply_label_side(arena, back, LabelSide::Below);
    } else if dummy_nodes.len() == 2 {
        let first = dummy_nodes.pop_front().unwrap();
        let second = dummy_nodes.pop_front().unwrap();
        apply_label_side(arena, first, LabelSide::Above);
        apply_label_side(arena, second, LabelSide::Below);
    } else {
        apply_for_dummy_node_run_with_simple_loops(arena, dummy_nodes, default_side);
    }

    dummy_nodes.clear();
}

/// Java `LabelSideSelector.applyForDummyNodeRunWithSimpleLoops`.
fn apply_for_dummy_node_run_with_simple_loops(
    arena: &mut LGraphArena,
    dummy_nodes: &VecDeque<LNodeId>,
    default_side: LabelSide,
) {
    let mut label_dummy_run: Vec<LNodeId> = Vec::with_capacity(dummy_nodes.len());
    let mut prev_source: Option<LNodeId> = None;
    let mut prev_target: Option<LNodeId> = None;

    for &current in dummy_nodes {
        debug_assert!(matches!(
            arena.nodes[current.0].node_type,
            NodeType::Label | NodeType::LongEdge
        ));
        let curr_source = arena.nodes[current.0]
            .props
            .long_edge_source
            .and_then(|p| arena.ports[p.0].owner);
        let curr_target = arena.nodes[current.0]
            .props
            .long_edge_target
            .and_then(|p| arena.ports[p.0].owner);

        if prev_source != curr_source || prev_target != curr_target {
            apply_label_sides_to_label_dummy_run(arena, &mut label_dummy_run, default_side);
            prev_source = curr_source;
            prev_target = curr_target;
        }
        label_dummy_run.push(current);
    }
    apply_label_sides_to_label_dummy_run(arena, &mut label_dummy_run, default_side);
}

/// Java `LabelSideSelector.applyLabelSidesToLabelDummyRun`.
fn apply_label_sides_to_label_dummy_run(
    arena: &mut LGraphArena,
    label_dummy_run: &mut Vec<LNodeId>,
    default_side: LabelSide,
) {
    if !label_dummy_run.is_empty() {
        if label_dummy_run.len() == 2 {
            apply_label_side(arena, label_dummy_run[0], LabelSide::Above);
            apply_label_side(arena, label_dummy_run[1], LabelSide::Below);
        } else {
            for &dummy in label_dummy_run.iter() {
                apply_label_side(arena, dummy, default_side);
            }
        }
        label_dummy_run.clear();
    }
}

/// Java `LabelSideSelector.applyLabelSide(LNode, LabelSide)`: annotate the
/// label dummy and move its ports so the dummy's box extends to the
/// correct side of its edge. (The inline branch is faithful but
/// unreachable through draw-uml inputs — see `LabelProps::inline`.)
fn apply_label_side(arena: &mut LGraphArena, label_dummy: LNodeId, side: LabelSide) {
    if arena.nodes[label_dummy.0].node_type != NodeType::Label {
        return;
    }
    let effective_side =
        if is_inline_edge_label(arena, label_dummy) { LabelSide::Inline } else { side };
    arena.nodes[label_dummy.0].props.label_side = effective_side;

    if effective_side != LabelSide::Below {
        // EDGE_THICKNESS of the origin edge — always the 1.0 default.
        let thickness = 1.0f64;
        let mut port_pos = 0.0;
        if effective_side == LabelSide::Above {
            port_pos = arena.nodes[label_dummy.0].size.y - (thickness / 2.0).ceil();
        } else if effective_side == LabelSide::Inline {
            let graph = arena.nodes[label_dummy.0].graph;
            let edge_label_spacing = arena.graphs[graph.0].props.spacing.edge_label;
            port_pos = (arena.nodes[label_dummy.0].size.y - edge_label_spacing - thickness)
                .ceil()
                / 2.0;
            arena.nodes[label_dummy.0].size.y -= edge_label_spacing;
            arena.nodes[label_dummy.0].size.y -= thickness;
        }
        for port in arena.nodes[label_dummy.0].ports.clone() {
            arena.ports[port.0].position.y = port_pos;
        }
    }
}

// ----------------------------------------------------------------------
// LabelDummyRemover (after phase 5, after LongEdgeJoiner)
// ----------------------------------------------------------------------

/// Port of `org.eclipse.elk.alg.layered.intermediate.LabelDummyRemover`.
/// Places each label dummy's represented labels at the dummy's final
/// box (respecting the chosen label side), re-attaches them to the
/// original edge, joins the two edge halves around the dummy, and drops
/// the dummy from its layer.
///
/// Label positions are written in the internal rightward frame (like
/// node positions); label extents stay in the user frame, so the
/// vertical branch swaps `size.x`/`size.y` relative to the Java source.
pub fn label_dummy_remover(arena: &mut LGraphArena, graph: LGraphId) {
    let edge_label_spacing = arena.graphs[graph.0].props.spacing.edge_label;
    let label_label_spacing = arena.graphs[graph.0].props.spacing.label_label;
    let direction = arena.graphs[graph.0].props.direction;
    let vertical = matches!(direction, Direction::Up | Direction::Down);

    for li in 0..arena.graphs[graph.0].layers.len() {
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            if arena.nodes[node.0].node_type != NodeType::Label {
                continue;
            }
            // EDGE_THICKNESS of the origin edge — the 1.0 default.
            let thickness = 1.0f64;
            let labels_below_edge =
                arena.nodes[node.0].props.label_side == LabelSide::Below;

            let mut curr_label_pos = arena.nodes[node.0].position;
            if labels_below_edge {
                curr_label_pos.y += thickness + edge_label_spacing;
            }
            let inline_node = is_inline_edge_label(arena, node);
            let label_space = KVector::new(
                arena.nodes[node.0].size.x,
                arena.nodes[node.0].size.y
                    + if inline_node { 0.0 } else { -thickness - edge_label_spacing },
            );

            let represented = arena.nodes[node.0].props.represented_labels.clone();
            if vertical {
                place_labels_for_vertical_layout(
                    arena,
                    &represented,
                    curr_label_pos,
                    label_label_spacing,
                    label_space,
                    labels_below_edge,
                    direction,
                );
            } else {
                place_labels_for_horizontal_layout(
                    arena,
                    &represented,
                    curr_label_pos,
                    label_label_spacing,
                    label_space,
                );
            }

            // Add represented labels back to the original edge.
            let origin_edge = arena.nodes[node.0]
                .props
                .origin_edge
                .expect("label dummy remembers its origin edge");
            arena.edges[origin_edge.0].labels.extend(represented);

            // Join the surviving halves (no unnecessary bend points —
            // edge routing is ORTHOGONAL in scope, not POLYLINE).
            join_at(arena, node);
            arena.node_set_layer(graph, node, None);
        }
    }
}

/// Java `LabelDummyRemover.placeLabelsForHorizontalLayout` (sizes are
/// untransposed on this branch).
fn place_labels_for_horizontal_layout(
    arena: &mut LGraphArena,
    labels: &[LLabelId],
    mut label_pos: KVector,
    label_spacing: f64,
    label_space: KVector,
) {
    for &label in labels {
        let size = arena.labels[label.0].size;
        arena.labels[label.0].position.x = label_pos.x + (label_space.x - size.x) / 2.0;
        arena.labels[label.0].position.y = label_pos.y;
        label_pos.y += size.y + label_spacing;
    }
}

/// Java `LabelDummyRemover.placeLabelsForVerticalLayout`. Internal label
/// extents are the transposed user size: internal x = user height,
/// internal y = user width.
fn place_labels_for_vertical_layout(
    arena: &mut LGraphArena,
    labels: &[LLabelId],
    mut label_pos: KVector,
    label_spacing: f64,
    label_space: KVector,
    left_aligned: bool,
    direction: Direction,
) {
    // Alignment is overridden if all labels here are inline labels.
    let inline = labels.iter().all(|&l| arena.labels[l.0].props.inline);

    // For UP layouts the label list is traversed in reverse to keep the
    // final label order.
    let effective: Vec<LLabelId> = if direction == Direction::Up {
        labels.iter().rev().copied().collect()
    } else {
        labels.to_vec()
    };

    for label in effective {
        let user = arena.labels[label.0].size;
        let (internal_x, internal_y) = (user.y, user.x);
        arena.labels[label.0].position.x = label_pos.x;
        arena.labels[label.0].position.y = if inline {
            label_pos.y + (label_space.y - internal_y) / 2.0
        } else if left_aligned {
            label_pos.y
        } else {
            label_pos.y + label_space.y - internal_y
        };
        label_pos.x += internal_x + label_spacing;
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

        // Join their labels (junction points never occur in scope).
        let dropped_labels = std::mem::take(&mut arena.edges[dropped.0].labels);
        arena.edges[surviving.0].labels.extend(dropped_labels);
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
