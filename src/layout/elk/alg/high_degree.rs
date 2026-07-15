//! Port of `org.eclipse.elk.alg.layered.intermediate.HighDegreeNodeLayeringProcessor`
//! (runs between phase 2 and phase 3 when
//! `highDegreeNodes.treatment = true`).
//!
//! For every node whose degree reaches `highDegreeNodes.threshold`,
//! the *trees* hanging off it (incoming and outgoing) are pulled out
//! of the regular layering and placed into freshly inserted layers
//! directly before/after the high-degree node's layer — so a hub's
//! leaf paraphernalia hugs it instead of stretching the main layering.

use super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId};

struct HighDegreeNodeInformation {
    inc_trees_max_height: i32,
    inc_tree_roots: Vec<LNodeId>,
    out_trees_max_height: i32,
    out_tree_roots: Vec<LNodeId>,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Incoming,
    Outgoing,
}

/// Java `HighDegreeNodeLayeringProcessor.process(graph)`.
pub fn process(arena: &mut LGraphArena, graph: LGraphId) {
    let degree_threshold = arena.graphs[graph.0].props.high_degree_nodes_threshold;
    let mut tree_height_threshold = arena.graphs[graph.0].props.high_degree_nodes_tree_height;
    if tree_height_threshold == 0 {
        tree_height_threshold = i32::MAX;
    }

    // The Java ListIterator walk with in-place layer insertion becomes
    // an index walk; inserted layers adjust the cursor the same way
    // (appended layers are skipped, prepended layers advance it).
    let mut layer_index = 0usize;
    while layer_index < arena.graphs[graph.0].layers.len() {
        let layer_nodes = arena.graphs[graph.0].layers[layer_index].nodes.clone();
        let mut high_degree_nodes: Vec<(LNodeId, HighDegreeNodeInformation)> = Vec::new();
        let mut inc_max = -1i32;
        let mut out_max = -1i32;
        for n in layer_nodes {
            if is_high_degree_node(arena, n, degree_threshold) {
                let hdni = calculate_information(arena, n, degree_threshold, tree_height_threshold);
                inc_max = inc_max.max(hdni.inc_trees_max_height);
                out_max = out_max.max(hdni.out_trees_max_height);
                high_degree_nodes.push((n, hdni));
            }
        }

        // Pre-layers: inserted directly before this layer; list built
        // nearest-first (Java `preLayers.add(0, prependLayer(..))`).
        let mut pre_layers: Vec<usize> = Vec::new();
        for _ in 0..inc_max.max(0) {
            arena.insert_layer(graph, layer_index);
            layer_index += 1;
            // Every previously inserted pre-layer shifted right by one.
            for l in pre_layers.iter_mut() {
                *l += 1;
            }
            pre_layers.insert(0, layer_index - 1);
        }
        for (_, hdni) in &high_degree_nodes {
            for &inc_root in &hdni.inc_tree_roots {
                move_tree(arena, graph, inc_root, Dir::Incoming, &pre_layers);
            }
        }

        // After-layers: appended directly after this layer, skipped by
        // the cursor (Java appendLayer leaves them behind the
        // iterator).
        let mut after_layers: Vec<usize> = Vec::new();
        for i in 0..out_max.max(0) {
            arena.insert_layer(graph, layer_index + 1 + i as usize);
            after_layers.push(layer_index + 1 + i as usize);
        }
        for (_, hdni) in &high_degree_nodes {
            for &out_root in &hdni.out_tree_roots {
                move_tree(arena, graph, out_root, Dir::Outgoing, &after_layers);
            }
        }
        layer_index += 1 + after_layers.len();
    }

    arena.remove_empty_layers(graph);
}

fn degree(arena: &LGraphArena, node: LNodeId) -> usize {
    arena.node_connected_edges(node).len()
}

fn is_high_degree_node(arena: &LGraphArena, node: LNodeId, threshold: i32) -> bool {
    degree(arena, node) as i32 >= threshold
}

fn edges_of(arena: &LGraphArena, node: LNodeId, dir: Dir) -> Vec<LEdgeId> {
    match dir {
        Dir::Incoming => arena.node_incoming_edges(node),
        Dir::Outgoing => arena.node_outgoing_edges(node),
    }
}

fn other(arena: &LGraphArena, edge: LEdgeId, node: LNodeId) -> LNodeId {
    let src = arena.edge_source_node(edge).unwrap();
    if src == node {
        arena.edge_target_node(edge).unwrap()
    } else {
        src
    }
}

/// Java `hasSingleConnection(node, edgeSelector)` — all edges of the
/// given direction lead to one and the same neighbor.
fn has_single_connection(arena: &LGraphArena, node: LNodeId, dir: Dir) -> bool {
    let mut connection: Option<LNodeId> = None;
    for e in edges_of(arena, node, dir) {
        let o = other(arena, e, node);
        match connection {
            None => connection = Some(o),
            Some(c) if c != o => return false,
            _ => {}
        }
    }
    true
}

/// Java `isTreeRoot(root, ancestorEdges, descendantEdges)` → height of
/// the tree, or -1 if it is not a tree.
fn is_tree_root(
    arena: &LGraphArena,
    root: LNodeId,
    ancestor_dir: Dir,
    descendant_dir: Dir,
    degree_threshold: i32,
    tree_height_threshold: i32,
) -> i32 {
    if is_high_degree_node(arena, root, degree_threshold) {
        return -1;
    }
    if !has_single_connection(arena, root, ancestor_dir) {
        return -1;
    }
    let descendants = edges_of(arena, root, descendant_dir);
    if descendants.is_empty() {
        return 1;
    }
    let mut current_height = 0;
    for e in descendants {
        let o = other(arena, e, root);
        let height = is_tree_root(
            arena,
            o,
            ancestor_dir,
            descendant_dir,
            degree_threshold,
            tree_height_threshold,
        );
        if height == -1 {
            return -1;
        }
        current_height = current_height.max(height);
        if current_height > tree_height_threshold - 1 {
            return -1;
        }
    }
    current_height + 1
}

/// Java `calculateInformation(hdn)`.
fn calculate_information(
    arena: &LGraphArena,
    hdn: LNodeId,
    degree_threshold: i32,
    tree_height_threshold: i32,
) -> HighDegreeNodeInformation {
    let mut hdni = HighDegreeNodeInformation {
        inc_trees_max_height: -1,
        inc_tree_roots: Vec::new(),
        out_trees_max_height: -1,
        out_tree_roots: Vec::new(),
    };
    for inc_edge in arena.node_incoming_edges(hdn) {
        if arena.edge_is_self_loop(inc_edge) {
            continue;
        }
        let src = arena.edge_source_node(inc_edge).unwrap();
        if has_single_connection(arena, src, Dir::Outgoing) {
            let tree_height = is_tree_root(
                arena,
                src,
                Dir::Outgoing,
                Dir::Incoming,
                degree_threshold,
                tree_height_threshold,
            );
            if tree_height == -1 {
                continue;
            }
            hdni.inc_trees_max_height = hdni.inc_trees_max_height.max(tree_height);
            hdni.inc_tree_roots.push(src);
        }
    }
    for out_edge in arena.node_outgoing_edges(hdn) {
        if arena.edge_is_self_loop(out_edge) {
            continue;
        }
        let tgt = arena.edge_target_node(out_edge).unwrap();
        if has_single_connection(arena, tgt, Dir::Incoming) {
            let tree_height = is_tree_root(
                arena,
                tgt,
                Dir::Incoming,
                Dir::Outgoing,
                degree_threshold,
                tree_height_threshold,
            );
            if tree_height == -1 {
                continue;
            }
            hdni.out_trees_max_height = hdni.out_trees_max_height.max(tree_height);
            hdni.out_tree_roots.push(tgt);
        }
    }
    hdni
}

/// Java `moveTree(root, edgesFun, layers)` — root into `layers[0]`,
/// children (following `dir` away from the high-degree node) into the
/// remaining layers recursively.
fn move_tree(
    arena: &mut LGraphArena,
    graph: LGraphId,
    root: LNodeId,
    dir: Dir,
    layers: &[usize],
) {
    assert!(!layers.is_empty());
    arena.node_set_layer(graph, root, Some(layers[0]));
    for e in edges_of(arena, root, dir) {
        let o = other(arena, e, root);
        move_tree(arena, graph, o, dir, &layers[1..]);
    }
}
