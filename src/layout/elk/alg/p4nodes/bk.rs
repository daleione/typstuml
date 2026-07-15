//! Port of `org.eclipse.elk.alg.layered.p4nodes.bk` — the Brandes-Köpf
//! node placer (phase 4), which assigns each node its in-layer
//! coordinate (`node.position.y` in the internal rightward orientation).
//! EPL-2.0 (see `../LICENSE.md`).
//!
//! Collapses `BKNodePlacer` + `NeighborhoodInformation` + `BKAligner` +
//! `BKCompactor` + `SimpleThresholdStrategy` + `BKAlignedLayout` into one
//! module over the arena. Default config: `fixedAlignment = BALANCED`
//! (four layouts RIGHT/LEFT × DOWN/UP, then the median) and
//! `edgeStraightening = IMPROVE_STRAIGHTNESS` (→ `SimpleThresholdStrategy`).
//!
//! Scope: flat graph, no in-layer edges / self-loops / north-south, all
//! `PRIORITY_STRAIGHTNESS` = 0.

use std::collections::HashSet;

use super::super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId, LPortId, NodeType};
use super::super::spacings::vertical_spacing;

#[derive(Clone, Copy, PartialEq, Eq)]
#[derive(Debug)]
enum VDir {
    Down,
    Up,
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[derive(Debug)]
enum HDir {
    Right,
    Left,
}

/// Neighborhood information (`NeighborhoodInformation`): assigns global
/// node ids + per-layer node indices and records straightness-priority
/// left/right neighbors.
struct Ni {
    node_count: usize,
    /// index of a node within its layer, by global node id.
    node_index: Vec<usize>,
    /// left (upper) neighbors: (node, connecting edge), by node id.
    left_neighbors: Vec<Vec<(LNodeId, LEdgeId)>>,
    /// right (lower) neighbors, by node id.
    right_neighbors: Vec<Vec<(LNodeId, LEdgeId)>>,
}

impl Ni {
    fn build(arena: &mut LGraphArena, graph: LGraphId) -> Ni {
        let layers: Vec<Vec<LNodeId>> =
            arena.graphs[graph.0].layers.iter().map(|l| l.nodes.clone()).collect();
        let mut node_count = 0;
        for l in &layers {
            node_count += l.len();
        }
        // Assign global ids (Java: n.id = nId++) and per-layer indices.
        let mut node_index = vec![0usize; node_count];
        let mut nid = 0usize;
        for l in &layers {
            for (n_index, &n) in l.iter().enumerate() {
                arena.nodes[n.0].id = nid;
                node_index[nid] = n_index;
                nid += 1;
            }
        }
        let mut ni = Ni {
            node_count,
            node_index,
            left_neighbors: vec![Vec::new(); node_count],
            right_neighbors: vec![Vec::new(); node_count],
        };
        // left/right neighbors (PRIORITY_STRAIGHTNESS all 0 → keep all,
        // sorted by neighbor's in-layer index).
        for l in &layers {
            for &n in l {
                let mut right = Vec::new();
                for e in arena.node_outgoing_edges(n) {
                    if arena.edge_is_self_loop(e) || is_in_layer_edge(arena, e) {
                        continue;
                    }
                    let t = arena.edge_target_node(e).unwrap();
                    right.push((t, e));
                }
                right.sort_by_key(|&(t, _)| ni.node_index[arena.nodes[t.0].id]);
                ni.right_neighbors[arena.nodes[n.0].id] = right;

                let mut left = Vec::new();
                for e in arena.node_incoming_edges(n) {
                    if arena.edge_is_self_loop(e) || is_in_layer_edge(arena, e) {
                        continue;
                    }
                    let s = arena.edge_source_node(e).unwrap();
                    left.push((s, e));
                }
                left.sort_by_key(|&(s, _)| ni.node_index[arena.nodes[s.0].id]);
                ni.left_neighbors[arena.nodes[n.0].id] = left;
            }
        }
        ni
    }
}

/// `BKAlignedLayout`.
struct Layout {
    root: Vec<LNodeId>,
    block_size: Vec<f64>,
    align: Vec<LNodeId>,
    inner_shift: Vec<f64>,
    sink: Vec<LNodeId>,
    shift: Vec<f64>,
    y: Vec<Option<f64>>,
    vdir: Option<VDir>,
    hdir: Option<HDir>,
    su: Vec<bool>,
    od: Vec<bool>,
}

impl Layout {
    fn new(n: usize, vdir: Option<VDir>, hdir: Option<HDir>, dummy: LNodeId) -> Layout {
        Layout {
            root: vec![dummy; n],
            block_size: vec![0.0; n],
            align: vec![dummy; n],
            inner_shift: vec![0.0; n],
            sink: vec![dummy; n],
            shift: vec![0.0; n],
            y: vec![None; n],
            vdir,
            hdir,
            su: vec![false; n],
            od: vec![true; n],
        }
    }
}

// ---- arena helpers ---------------------------------------------------

fn is_in_layer_edge(arena: &LGraphArena, e: LEdgeId) -> bool {
    let s = arena.edge_source_node(e).unwrap();
    let t = arena.edge_target_node(e).unwrap();
    arena.nodes[s.0].layer == arena.nodes[t.0].layer
}

fn gid(arena: &LGraphArena, n: LNodeId) -> usize {
    arena.nodes[n.0].id
}
fn size_y(arena: &LGraphArena, n: LNodeId) -> f64 {
    arena.nodes[n.0].size.y
}
fn margin_top(arena: &LGraphArena, n: LNodeId) -> f64 {
    arena.nodes[n.0].margin.top
}
fn margin_bottom(arena: &LGraphArena, n: LNodeId) -> f64 {
    arena.nodes[n.0].margin.bottom
}
fn port_pos_anchor_y(arena: &LGraphArena, p: LPortId) -> f64 {
    arena.ports[p.0].position.y + arena.ports[p.0].anchor.y
}
fn port_node(arena: &LGraphArena, p: LPortId) -> LNodeId {
    arena.ports[p.0].owner.unwrap()
}

/// Java `BKNodePlacer.getEdge(source, target)`.
fn get_edge(arena: &LGraphArena, source: LNodeId, target: LNodeId) -> Option<LEdgeId> {
    for e in arena.node_connected_edges(source) {
        let t = arena.edge_target_node(e).unwrap();
        let s = arena.edge_source_node(e).unwrap();
        if t == target || s == target {
            return Some(e);
        }
    }
    None
}

// ---- public entry ----------------------------------------------------

/// Java `BKNodePlacer.process`. Assigns `node.position.y` for every node.
pub fn place(arena: &mut LGraphArena, graph: LGraphId) {
    let n_layers = arena.graphs[graph.0].layers.len();
    if arena.graphs[graph.0].layers.iter().all(|l| l.nodes.is_empty()) {
        return;
    }
    let ni = Ni::build(arena, graph);
    let dummy = arena.graphs[graph.0].layers.iter().flat_map(|l| l.nodes.iter()).copied().next().unwrap();

    // Type-1 conflicts (direction-independent).
    let marked = mark_conflicts(arena, graph, &ni);

    // BALANCED (default) → all four layouts.
    let combos = [
        (VDir::Down, HDir::Right),
        (VDir::Up, HDir::Right),
        (VDir::Down, HDir::Left),
        (VDir::Up, HDir::Left),
    ];
    let mut layouts: Vec<Layout> = Vec::new();
    for (v, h) in combos {
        let mut bal = Layout::new(ni.node_count, Some(v), Some(h), dummy);
        vertical_alignment(arena, graph, &ni, &mut bal, &marked);
        inside_block_shift(arena, graph, &ni, &mut bal);
        layouts.push(bal);
    }
    let spacing = arena.graphs[graph.0].props.spacing;
    for bal in &mut layouts {
        horizontal_compaction(arena, graph, &ni, bal, &spacing);
    }

    // Java: `produceBalancedLayout = (align == NONE && !favorStraightEdges)
    // || align == BALANCED`. `favorStraightEdges` defaults to true, so an
    // option-less graph (compound children) skips the balanced median and
    // picks the smallest feasible of the four sweeps.
    let align = arena.graphs[graph.0].props.bk_fixed_alignment;
    assert!(
        matches!(align, super::super::options::FixedAlignment::None
            | super::super::options::FixedAlignment::Balanced),
        "single-sweep fixedAlignment values are outside the ported scope"
    );
    let produce_balanced = (align == super::super::options::FixedAlignment::None
        && !arena.graphs[graph.0].props.favor_straight_edges)
        || align == super::super::options::FixedAlignment::Balanced;

    let mut chosen = if produce_balanced {
        create_balanced_layout(arena, graph, &layouts, ni.node_count, dummy)
    } else {
        None
    };
    if chosen.is_some() && !check_order_constraint(arena, graph, &chosen) {
        chosen = None;
    }
    let chosen = match chosen {
        Some(c) => c,
        None => {
            // Smallest feasible of the four, else the first.
            let mut best: Option<usize> = None;
            for (i, bal) in layouts.iter().enumerate() {
                if check_order_constraint(arena, graph, &Some(clone_layout(bal)))
                    && (best.is_none()
                        || layout_size(arena, graph, &layouts[best.unwrap()])
                            > layout_size(arena, graph, bal))
                {
                    best = Some(i);
                }
            }
            clone_layout(&layouts[best.unwrap_or(0)])
        }
    };

    // Apply.
    for l in 0..n_layers {
        for node in arena.graphs[graph.0].layers[l].nodes.clone() {
            let id = gid(arena, node);
            let y = chosen.y[id].unwrap() + chosen.inner_shift[id];
            arena.nodes[node.0].position.y = y;
        }
    }
    if std::env::var("ELK_DBG").is_ok() {
        let mut real = 0usize;
        let mut rows: Vec<String> = Vec::new();
        for (li, layer) in arena.graphs[graph.0].layers.iter().enumerate() {
            let row: Vec<String> = layer
                .nodes
                .iter()
                .map(|&n| {
                    if arena.nodes[n.0].node_type == super::super::graph::NodeType::Normal {
                        real += 1;
                    }
                    let name = arena.nodes[n.0]
                        .props
                        .origin
                        .clone()
                        .unwrap_or_else(|| format!("T{:?}", arena.nodes[n.0].node_type));
                    format!("{name}={:.6}", arena.nodes[n.0].position.y)
                })
                .collect();
            rows.push(format!("L{li}[{}]", row.join(" ")));
        }
        eprintln!("BKY real={real} {}", rows.join(" | "));
        for layout in &layouts {
            let mut ys: Vec<String> = Vec::new();
            for layer in &arena.graphs[graph.0].layers {
                for &n in &layer.nodes {
                    let id = gid(arena, n);
                    ys.push(format!("{:.4}", layout.y[id].unwrap_or(0.0) + layout.inner_shift[id]));
                }
            }
            eprintln!("BKL {:?}/{:?} [{}]", layout.vdir, layout.hdir, ys.join(" "));
        }
    }
}

fn clone_layout(l: &Layout) -> Layout {
    Layout {
        root: l.root.clone(),
        block_size: l.block_size.clone(),
        align: l.align.clone(),
        inner_shift: l.inner_shift.clone(),
        sink: l.sink.clone(),
        shift: l.shift.clone(),
        y: l.y.clone(),
        vdir: l.vdir,
        hdir: l.hdir,
        su: l.su.clone(),
        od: l.od.clone(),
    }
}

// ---- conflict marking ------------------------------------------------

fn incident_to_inner_segment(
    arena: &LGraphArena,
    node: LNodeId,
    layer1: usize,
    layer2: usize,
) -> bool {
    if arena.nodes[node.0].node_type != NodeType::LongEdge {
        return false;
    }
    for e in arena.node_incoming_edges(node) {
        let s = arena.edge_source_node(e).unwrap();
        if arena.nodes[s.0].node_type == NodeType::LongEdge
            && arena.nodes[s.0].layer == Some(layer2)
            && arena.nodes[node.0].layer == Some(layer1)
        {
            return true;
        }
    }
    false
}

fn mark_conflicts(arena: &LGraphArena, graph: LGraphId, ni: &Ni) -> HashSet<LEdgeId> {
    let mut marked = HashSet::new();
    let layers: Vec<Vec<LNodeId>> =
        arena.graphs[graph.0].layers.iter().map(|l| l.nodes.clone()).collect();
    let number_of_layers = layers.len();
    if number_of_layers < 3 {
        return marked;
    }
    let layer_size: Vec<usize> = layers.iter().map(|l| l.len()).collect();
    for i in 1..number_of_layers - 1 {
        // ELK iterates layers from index 2, so `currentLayer` is layer i+1.
        let current_layer = &layers[i + 1];
        let mut k_0 = 0i64;
        let mut l = 0usize;
        for l_1 in 0..layer_size[i + 1] {
            let v_l_i = current_layer[l_1];
            if l_1 == layer_size[i + 1] - 1 || incident_to_inner_segment(arena, v_l_i, i + 1, i) {
                let mut k_1 = (layer_size[i] - 1) as i64;
                if incident_to_inner_segment(arena, v_l_i, i + 1, i) {
                    let ln = ni.left_neighbors[gid(arena, v_l_i)][0].0;
                    k_1 = ni.node_index[gid(arena, ln)] as i64;
                }
                while l <= l_1 {
                    let v_l = current_layer[l];
                    if !incident_to_inner_segment(arena, v_l, i + 1, i) {
                        for &(upper, edge) in &ni.left_neighbors[gid(arena, v_l)] {
                            let k = ni.node_index[gid(arena, upper)] as i64;
                            if k < k_0 || k > k_1 {
                                marked.insert(edge);
                            }
                        }
                    }
                    l += 1;
                }
                k_0 = k_1;
            }
        }
    }
    marked
}

// ---- alignment -------------------------------------------------------

#[allow(clippy::needless_range_loop)] // faithful port of ELK's indexed neighbor scan
fn vertical_alignment(
    arena: &LGraphArena,
    graph: LGraphId,
    ni: &Ni,
    bal: &mut Layout,
    marked: &HashSet<LEdgeId>,
) {
    for l in 0..arena.graphs[graph.0].layers.len() {
        for &v in &arena.graphs[graph.0].layers[l].nodes {
            let id = gid(arena, v);
            bal.root[id] = v;
            bal.align[id] = v;
            bal.inner_shift[id] = 0.0;
        }
    }
    let mut layer_order: Vec<usize> = (0..arena.graphs[graph.0].layers.len()).collect();
    if bal.hdir == Some(HDir::Left) {
        layer_order.reverse();
    }
    for &li in &layer_order {
        let mut r: i64 = -1;
        let mut nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        if bal.vdir == Some(VDir::Up) {
            r = i64::MAX;
            nodes.reverse();
        }
        for v_i_k in nodes {
            let vid = gid(arena, v_i_k);
            let neighbors = if bal.hdir == Some(HDir::Left) {
                &ni.right_neighbors[vid]
            } else {
                &ni.left_neighbors[vid]
            };
            let d = neighbors.len();
            if d == 0 {
                continue;
            }
            let low = (((d as f64 + 1.0) / 2.0).floor() as i64 - 1).max(0) as usize;
            let high = ((d as f64 + 1.0) / 2.0).ceil() as i64 - 1;
            let high = high.max(0) as usize;
            if bal.vdir == Some(VDir::Up) {
                let mut m = high as i64;
                while m >= low as i64 {
                    if bal.align[vid] == v_i_k {
                        let (u_m, edge) = neighbors[m as usize];
                        let u_m_index = ni.node_index[gid(arena, u_m)] as i64;
                        if !marked.contains(&edge) && r > u_m_index {
                            let umid = gid(arena, u_m);
                            bal.align[umid] = v_i_k;
                            bal.root[vid] = bal.root[umid];
                            bal.align[vid] = bal.root[vid];
                            let rootid = gid(arena, bal.root[vid]);
                            bal.od[rootid] &= arena.nodes[v_i_k.0].node_type == NodeType::LongEdge;
                            r = u_m_index;
                        }
                    }
                    m -= 1;
                }
            } else {
                for m in low..=high {
                    if bal.align[vid] == v_i_k {
                        let (um, edge) = neighbors[m];
                        let um_index = ni.node_index[gid(arena, um)] as i64;
                        if !marked.contains(&edge) && r < um_index {
                            let umid = gid(arena, um);
                            bal.align[umid] = v_i_k;
                            bal.root[vid] = bal.root[umid];
                            bal.align[vid] = bal.root[vid];
                            let rootid = gid(arena, bal.root[vid]);
                            bal.od[rootid] &= arena.nodes[v_i_k.0].node_type == NodeType::LongEdge;
                            r = um_index;
                        }
                    }
                }
            }
        }
    }
}

fn get_blocks(arena: &LGraphArena, graph: LGraphId, bal: &Layout) -> Vec<(LNodeId, Vec<LNodeId>)> {
    // Preserve insertion order (LinkedHashMap): roots in layer/node order.
    let mut roots: Vec<LNodeId> = Vec::new();
    let mut contents: std::collections::HashMap<LNodeId, Vec<LNodeId>> = Default::default();
    for l in 0..arena.graphs[graph.0].layers.len() {
        for &node in &arena.graphs[graph.0].layers[l].nodes {
            let root = bal.root[gid(arena, node)];
            contents.entry(root).or_insert_with(|| {
                roots.push(root);
                Vec::new()
            });
            contents.get_mut(&root).unwrap().push(node);
        }
    }
    roots.into_iter().map(|r| (r, contents.remove(&r).unwrap())).collect()
}

fn inside_block_shift(arena: &LGraphArena, graph: LGraphId, _ni: &Ni, bal: &mut Layout) {
    let blocks = get_blocks(arena, graph, bal);
    for (root, _) in blocks {
        let root_id = gid(arena, root);
        let mut space_above = margin_top(arena, root);
        let mut space_below = size_y(arena, root) + margin_bottom(arena, root);
        bal.inner_shift[root_id] = 0.0;

        let mut current = root;
        loop {
            let next = bal.align[gid(arena, current)];
            if next == root {
                break;
            }
            let edge = get_edge(arena, current, next).unwrap();
            let (sp, tp) = (arena.edges[edge.0].source.unwrap(), arena.edges[edge.0].target.unwrap());
            let port_pos_diff = if bal.hdir == Some(HDir::Left) {
                port_pos_anchor_y(arena, tp) - port_pos_anchor_y(arena, sp)
            } else {
                port_pos_anchor_y(arena, sp) - port_pos_anchor_y(arena, tp)
            };
            let next_inner_shift = bal.inner_shift[gid(arena, current)] + port_pos_diff;
            bal.inner_shift[gid(arena, next)] = next_inner_shift;
            space_above = space_above.max(margin_top(arena, next) - next_inner_shift);
            space_below =
                space_below.max(next_inner_shift + size_y(arena, next) + margin_bottom(arena, next));
            current = next;
        }

        let mut current = root;
        loop {
            let id = gid(arena, current);
            bal.inner_shift[id] += space_above;
            current = bal.align[id];
            if current == root {
                break;
            }
        }
        bal.block_size[root_id] = space_above + space_below;
    }
}

// ---- layout size / order check / delta / space helpers ---------------

fn layout_size(arena: &LGraphArena, graph: LGraphId, bal: &Layout) -> f64 {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for l in 0..arena.graphs[graph.0].layers.len() {
        for &n in &arena.graphs[graph.0].layers[l].nodes {
            let id = gid(arena, n);
            let y_min = bal.y[id].unwrap();
            let y_max = y_min + bal.block_size[gid(arena, bal.root[id])];
            min = min.min(y_min);
            max = max.max(y_max);
        }
    }
    max - min
}

fn get_min_y(arena: &LGraphArena, bal: &Layout, n: LNodeId) -> f64 {
    let id = gid(arena, n);
    let root = bal.root[id];
    bal.y[gid(arena, root)].unwrap() + bal.inner_shift[id] - margin_top(arena, n)
}
fn get_max_y(arena: &LGraphArena, bal: &Layout, n: LNodeId) -> f64 {
    let id = gid(arena, n);
    let root = bal.root[id];
    bal.y[gid(arena, root)].unwrap() + bal.inner_shift[id] + size_y(arena, n) + margin_bottom(arena, n)
}

fn upper_neighbor(arena: &LGraphArena, graph: LGraphId, ni: &Ni, n: LNodeId) -> Option<LNodeId> {
    let li = arena.nodes[n.0].layer.unwrap();
    let idx = ni.node_index[gid(arena, n)];
    if idx > 0 {
        Some(arena.graphs[graph.0].layers[li].nodes[idx - 1])
    } else {
        None
    }
}
fn lower_neighbor(arena: &LGraphArena, graph: LGraphId, ni: &Ni, n: LNodeId) -> Option<LNodeId> {
    let li = arena.nodes[n.0].layer.unwrap();
    let idx = ni.node_index[gid(arena, n)];
    if idx < arena.graphs[graph.0].layers[li].nodes.len() - 1 {
        Some(arena.graphs[graph.0].layers[li].nodes[idx + 1])
    } else {
        None
    }
}

fn calculate_delta(arena: &LGraphArena, bal: &Layout, src: LPortId, tgt: LPortId) -> f64 {
    let src_node = port_node(arena, src);
    let tgt_node = port_node(arena, tgt);
    let src_pos = bal.y[gid(arena, src_node)].unwrap()
        + bal.inner_shift[gid(arena, src_node)]
        + port_pos_anchor_y(arena, src);
    let tgt_pos = bal.y[gid(arena, tgt_node)].unwrap()
        + bal.inner_shift[gid(arena, tgt_node)]
        + port_pos_anchor_y(arena, tgt);
    tgt_pos - src_pos
}

fn shift_block(arena: &LGraphArena, bal: &mut Layout, root_node: LNodeId, delta: f64) {
    let mut current = root_node;
    loop {
        let id = gid(arena, current);
        bal.y[id] = Some(bal.y[id].unwrap() + delta);
        current = bal.align[id];
        if current == root_node {
            break;
        }
    }
}

fn check_space_above(
    arena: &LGraphArena,
    graph: LGraphId,
    ni: &Ni,
    bal: &Layout,
    spacing: &super::super::options::SpacingProps,
    block_root: LNodeId,
    delta: f64,
) -> f64 {
    let mut available = delta;
    let mut current = block_root;
    loop {
        current = bal.align[gid(arena, current)];
        let min_y_current = get_min_y(arena, bal, current);
        if let Some(neighbor) = upper_neighbor(arena, graph, ni, current) {
            let max_y_neighbor = get_max_y(arena, bal, neighbor);
            let s = vertical_spacing(
                spacing,
                arena.nodes[current.0].node_type,
                arena.nodes[neighbor.0].node_type,
            );
            available = available.min(min_y_current - (max_y_neighbor + s));
        }
        if block_root == current {
            break;
        }
    }
    available
}

fn check_space_below(
    arena: &LGraphArena,
    graph: LGraphId,
    ni: &Ni,
    bal: &Layout,
    spacing: &super::super::options::SpacingProps,
    block_root: LNodeId,
    delta: f64,
) -> f64 {
    let mut available = delta;
    let mut current = block_root;
    loop {
        current = bal.align[gid(arena, current)];
        let max_y_current = get_max_y(arena, bal, current);
        if let Some(neighbor) = lower_neighbor(arena, graph, ni, current) {
            let min_y_neighbor = get_min_y(arena, bal, neighbor);
            let s = vertical_spacing(
                spacing,
                arena.nodes[current.0].node_type,
                arena.nodes[neighbor.0].node_type,
            );
            available = available.min(min_y_neighbor - (max_y_current + s));
        }
        if block_root == current {
            break;
        }
    }
    available
}

fn check_order_constraint(arena: &LGraphArena, graph: LGraphId, bal: &Option<Layout>) -> bool {
    let Some(bal) = bal else { return false };
    for l in 0..arena.graphs[graph.0].layers.len() {
        let mut pos = f64::NEG_INFINITY;
        for &node in &arena.graphs[graph.0].layers[l].nodes {
            let id = gid(arena, node);
            let top = bal.y[id].unwrap() + bal.inner_shift[id] - margin_top(arena, node);
            let bottom =
                bal.y[id].unwrap() + bal.inner_shift[id] + size_y(arena, node) + margin_bottom(arena, node);
            if top > pos && bottom > pos {
                pos = bottom;
            } else {
                return false;
            }
        }
    }
    true
}

// ---- compaction (with SimpleThresholdStrategy) -----------------------

const THRESHOLD: f64 = f64::MAX;

struct ClassNode {
    class_shift: Option<f64>,
    node: LNodeId,
    outgoing: Vec<(usize, f64)>, // (target index into class_nodes, separation)
    indegree: i32,
}

struct Postprocessable {
    free: LNodeId,
    is_root: bool,
    has_edges: bool,
    edge: Option<LEdgeId>,
}

struct Compactor<'a> {
    arena: &'a LGraphArena,
    graph: LGraphId,
    ni: &'a Ni,
    spacing: super::super::options::SpacingProps,
    // class graph
    class_nodes: Vec<ClassNode>,
    class_of: std::collections::HashMap<LNodeId, usize>,
    // threshold strategy state
    block_finished: HashSet<LNodeId>,
    pp_queue: std::collections::VecDeque<Postprocessable>,
    pp_stack: Vec<Postprocessable>,
}

fn horizontal_compaction(
    arena: &LGraphArena,
    graph: LGraphId,
    ni: &Ni,
    bal: &mut Layout,
    spacing: &super::super::options::SpacingProps,
) {
    for l in 0..arena.graphs[graph.0].layers.len() {
        for &node in &arena.graphs[graph.0].layers[l].nodes {
            let id = gid(arena, node);
            bal.sink[id] = node;
            bal.shift[id] = if bal.vdir == Some(VDir::Up) { f64::NEG_INFINITY } else { f64::INFINITY };
        }
    }
    let mut c = Compactor {
        arena,
        graph,
        ni,
        spacing: *spacing,
        class_nodes: Vec::new(),
        class_of: Default::default(),
        block_finished: HashSet::new(),
        pp_queue: Default::default(),
        pp_stack: Vec::new(),
    };

    for v in bal.y.iter_mut() {
        *v = None;
    }
    let mut layer_order: Vec<usize> = (0..arena.graphs[graph.0].layers.len()).collect();
    if bal.hdir == Some(HDir::Left) {
        layer_order.reverse();
    }
    for &li in &layer_order {
        let mut nodes = arena.graphs[graph.0].layers[li].nodes.clone();
        if bal.vdir == Some(VDir::Up) {
            nodes.reverse();
        }
        for v in nodes {
            if bal.root[gid(arena, v)] == v {
                c.place_block(bal, v);
            }
        }
    }
    c.place_classes(bal);
    // apply final coordinates — ELK iterates the (LEFT-reversed) layer
    // list so each block's root is shifted before its members read it.
    for &li in &layer_order {
        for &v in &arena.graphs[graph.0].layers[li].nodes.clone() {
            let id = gid(arena, v);
            bal.y[id] = bal.y[gid(arena, bal.root[id])];
            if v == bal.root[id] {
                let sink_shift = bal.shift[gid(arena, bal.sink[id])];
                if (bal.vdir == Some(VDir::Up) && sink_shift > f64::NEG_INFINITY)
                    || (bal.vdir == Some(VDir::Down) && sink_shift < f64::INFINITY)
                {
                    bal.y[id] = Some(bal.y[id].unwrap() + sink_shift);
                }
            }
        }
    }
    c.post_process(bal);
}

impl<'a> Compactor<'a> {
    fn class_index(&mut self, sink: LNodeId) -> usize {
        if let Some(&i) = self.class_of.get(&sink) {
            return i;
        }
        let i = self.class_nodes.len();
        self.class_nodes.push(ClassNode { class_shift: None, node: sink, outgoing: Vec::new(), indegree: 0 });
        self.class_of.insert(sink, i);
        i
    }

    fn place_block(&mut self, bal: &mut Layout, root: LNodeId) {
        if bal.y[gid(self.arena, root)].is_some() {
            return;
        }
        let mut is_initial = true;
        bal.y[gid(self.arena, root)] = Some(0.0);
        let mut current = root;
        let mut thresh = if bal.vdir == Some(VDir::Down) { f64::NEG_INFINITY } else { f64::INFINITY };
        loop {
            let cur_idx = self.ni.node_index[gid(self.arena, current)];
            let layer = self.arena.nodes[current.0].layer.unwrap();
            let layer_size = self.arena.graphs[self.graph.0].layers[layer].nodes.len();
            if (bal.vdir == Some(VDir::Down) && cur_idx > 0)
                || (bal.vdir == Some(VDir::Up) && cur_idx < layer_size - 1)
            {
                let neighbor = if bal.vdir == Some(VDir::Up) {
                    self.arena.graphs[self.graph.0].layers[layer].nodes[cur_idx + 1]
                } else {
                    self.arena.graphs[self.graph.0].layers[layer].nodes[cur_idx - 1]
                };
                let neighbor_root = bal.root[gid(self.arena, neighbor)];
                self.place_block(bal, neighbor_root);
                thresh = self.calculate_threshold(bal, thresh, root, current);
                if bal.sink[gid(self.arena, root)] == root {
                    bal.sink[gid(self.arena, root)] = bal.sink[gid(self.arena, neighbor_root)];
                }
                if bal.sink[gid(self.arena, root)] == bal.sink[gid(self.arena, neighbor_root)] {
                    let s = vertical_spacing(
                        &self.spacing,
                        self.arena.nodes[current.0].node_type,
                        self.arena.nodes[neighbor.0].node_type,
                    );
                    if bal.vdir == Some(VDir::Up) {
                        let cur_block_pos = bal.y[gid(self.arena, root)].unwrap();
                        let new_pos = bal.y[gid(self.arena, neighbor_root)].unwrap()
                            + bal.inner_shift[gid(self.arena, neighbor)]
                            - margin_top(self.arena, neighbor)
                            - s
                            - margin_bottom(self.arena, current)
                            - size_y(self.arena, current)
                            - bal.inner_shift[gid(self.arena, current)];
                        bal.y[gid(self.arena, root)] = Some(if is_initial {
                            is_initial = false;
                            new_pos.min(thresh)
                        } else {
                            cur_block_pos.min(new_pos.min(thresh))
                        });
                    } else {
                        let cur_block_pos = bal.y[gid(self.arena, root)].unwrap();
                        let new_pos = bal.y[gid(self.arena, neighbor_root)].unwrap()
                            + bal.inner_shift[gid(self.arena, neighbor)]
                            + size_y(self.arena, neighbor)
                            + margin_bottom(self.arena, neighbor)
                            + s
                            + margin_top(self.arena, current)
                            - bal.inner_shift[gid(self.arena, current)];
                        bal.y[gid(self.arena, root)] = Some(if is_initial {
                            is_initial = false;
                            new_pos.max(thresh)
                        } else {
                            cur_block_pos.max(new_pos.max(thresh))
                        });
                    }
                } else {
                    let s = self.arena.graphs[self.graph.0].props.spacing.node_node;
                    let sink_node = bal.sink[gid(self.arena, root)];
                    let sink_i = self.class_index(sink_node);
                    let neighbor_sink = bal.sink[gid(self.arena, neighbor_root)];
                    let neighbor_i = self.class_index(neighbor_sink);
                    let required = if bal.vdir == Some(VDir::Up) {
                        bal.y[gid(self.arena, root)].unwrap()
                            + bal.inner_shift[gid(self.arena, current)]
                            + size_y(self.arena, current)
                            + margin_bottom(self.arena, current)
                            + s
                            - (bal.y[gid(self.arena, neighbor_root)].unwrap()
                                + bal.inner_shift[gid(self.arena, neighbor)]
                                - margin_top(self.arena, neighbor))
                    } else {
                        bal.y[gid(self.arena, root)].unwrap()
                            + bal.inner_shift[gid(self.arena, current)]
                            - margin_top(self.arena, current)
                            - bal.y[gid(self.arena, neighbor_root)].unwrap()
                            - bal.inner_shift[gid(self.arena, neighbor)]
                            - size_y(self.arena, neighbor)
                            - margin_bottom(self.arena, neighbor)
                            - s
                    };
                    self.class_nodes[neighbor_i].indegree += 1;
                    self.class_nodes[sink_i].outgoing.push((neighbor_i, required));
                }
            } else {
                thresh = self.calculate_threshold(bal, thresh, root, current);
            }
            current = bal.align[gid(self.arena, current)];
            if current == root {
                break;
            }
        }
        self.block_finished.insert(root);
    }

    fn place_classes(&mut self, bal: &mut Layout) {
        let mut sinks: std::collections::VecDeque<usize> = Default::default();
        for (i, cn) in self.class_nodes.iter().enumerate() {
            if cn.indegree == 0 {
                sinks.push_back(i);
            }
        }
        while let Some(n) = sinks.pop_front() {
            if self.class_nodes[n].class_shift.is_none() {
                self.class_nodes[n].class_shift = Some(0.0);
            }
            let base = self.class_nodes[n].class_shift.unwrap();
            let outgoing = self.class_nodes[n].outgoing.clone();
            for (target, separation) in outgoing {
                match self.class_nodes[target].class_shift {
                    None => self.class_nodes[target].class_shift = Some(base + separation),
                    Some(cur) => {
                        self.class_nodes[target].class_shift = Some(if bal.vdir == Some(VDir::Down) {
                            cur.min(base + separation)
                        } else {
                            cur.max(base + separation)
                        });
                    }
                }
                self.class_nodes[target].indegree -= 1;
                if self.class_nodes[target].indegree == 0 {
                    sinks.push_back(target);
                }
            }
        }
        for cn in &self.class_nodes {
            if let Some(cs) = cn.class_shift {
                bal.shift[gid(self.arena, cn.node)] = cs;
            }
        }
    }

    // -- SimpleThresholdStrategy --

    fn calculate_threshold(
        &mut self,
        bal: &mut Layout,
        old_thresh: f64,
        block_root: LNodeId,
        current_node: LNodeId,
    ) -> f64 {
        let is_root = block_root == current_node;
        let is_last = bal.align[gid(self.arena, current_node)] == block_root;
        if !(is_root || is_last) {
            return old_thresh;
        }
        let mut t = old_thresh;
        if is_root {
            t = self.get_bound(bal, block_root, true);
        }
        if t.is_infinite() && is_last {
            t = self.get_bound(bal, current_node, false);
        }
        t
    }

    fn pick_edge(&mut self, bal: &Layout, free: LNodeId, is_root: bool) -> Postprocessable {
        let edges: Vec<LEdgeId> = if is_root {
            if bal.hdir == Some(HDir::Right) {
                self.arena.node_incoming_edges(free)
            } else {
                self.arena.node_outgoing_edges(free)
            }
        } else if bal.hdir == Some(HDir::Left) {
            self.arena.node_incoming_edges(free)
        } else {
            self.arena.node_outgoing_edges(free)
        };
        let only_dummies = bal.od[gid(self.arena, bal.root[gid(self.arena, free)])];
        let mut has_edges = false;
        for e in edges {
            if !only_dummies && is_in_layer_edge(self.arena, e) {
                continue;
            }
            if bal.su[gid(self.arena, bal.root[gid(self.arena, free)])] {
                continue;
            }
            has_edges = true;
            let other = other_node(self.arena, e, free);
            if self.block_finished.contains(&bal.root[gid(self.arena, other)]) {
                return Postprocessable { free, is_root, has_edges: true, edge: Some(e) };
            }
        }
        Postprocessable { free, is_root, has_edges, edge: None }
    }

    fn get_bound(&mut self, bal: &mut Layout, block_node: LNodeId, is_root: bool) -> f64 {
        let invalid = if bal.vdir == Some(VDir::Up) { f64::INFINITY } else { f64::NEG_INFINITY };
        let pick = self.pick_edge(bal, block_node, is_root);
        if pick.edge.is_none() && pick.has_edges {
            self.pp_queue.push_back(pick);
            return invalid;
        }
        let Some(edge) = pick.edge else { return invalid };
        let left = self.arena.edges[edge.0].source.unwrap();
        let right = self.arena.edges[edge.0].target.unwrap();
        let threshold = if is_root {
            let (root_port, other_port) =
                if bal.hdir == Some(HDir::Right) { (right, left) } else { (left, right) };
            let other_root = bal.root[gid(self.arena, port_node(self.arena, other_port))];
            bal.y[gid(self.arena, other_root)].unwrap()
                + bal.inner_shift[gid(self.arena, port_node(self.arena, other_port))]
                + port_pos_anchor_y(self.arena, other_port)
                - bal.inner_shift[gid(self.arena, port_node(self.arena, root_port))]
                - port_pos_anchor_y(self.arena, root_port)
        } else {
            let (root_port, other_port) =
                if bal.hdir == Some(HDir::Left) { (right, left) } else { (left, right) };
            bal.y[gid(self.arena, bal.root[gid(self.arena, port_node(self.arena, other_port))])].unwrap()
                + bal.inner_shift[gid(self.arena, port_node(self.arena, other_port))]
                + port_pos_anchor_y(self.arena, other_port)
                - bal.inner_shift[gid(self.arena, port_node(self.arena, root_port))]
                - port_pos_anchor_y(self.arena, root_port)
        };
        bal.su[gid(self.arena, bal.root[gid(self.arena, port_node(self.arena, left))])] = true;
        bal.su[gid(self.arena, bal.root[gid(self.arena, port_node(self.arena, right))])] = true;
        threshold
    }

    fn post_process(&mut self, bal: &mut Layout) {
        while let Some(pp) = self.pp_queue.pop_front() {
            let pick = self.pick_edge(bal, pp.free, pp.is_root);
            let Some(edge) = pick.edge else { continue };
            let only_dummies = bal.od[gid(self.arena, bal.root[gid(self.arena, pp.free)])];
            if !only_dummies && is_in_layer_edge(self.arena, edge) {
                continue;
            }
            let pp2 = Postprocessable { free: pp.free, is_root: pp.is_root, has_edges: true, edge: Some(edge) };
            let moved = self.process(bal, &pp2);
            if !moved {
                self.pp_stack.push(pp2);
            }
        }
        while let Some(pp) = self.pp_stack.pop() {
            self.process(bal, &pp);
        }
    }

    fn process(&mut self, bal: &mut Layout, pp: &Postprocessable) -> bool {
        let edge = pp.edge.unwrap();
        let (fix, block) = if port_node(self.arena, self.arena.edges[edge.0].source.unwrap()) == pp.free {
            (self.arena.edges[edge.0].target.unwrap(), self.arena.edges[edge.0].source.unwrap())
        } else {
            (self.arena.edges[edge.0].source.unwrap(), self.arena.edges[edge.0].target.unwrap())
        };
        let delta = calculate_delta(self.arena, bal, fix, block);
        if delta > 0.0 && delta < THRESHOLD {
            let available = check_space_above(
                self.arena, self.graph, self.ni, bal, &self.spacing, port_node(self.arena, block), delta,
            );
            shift_block(self.arena, bal, port_node(self.arena, block), -available);
            available > 0.0
        } else if delta < 0.0 && -delta < THRESHOLD {
            let available = check_space_below(
                self.arena, self.graph, self.ni, bal, &self.spacing, port_node(self.arena, block), -delta,
            );
            shift_block(self.arena, bal, port_node(self.arena, block), available);
            available > 0.0
        } else {
            false
        }
    }
}

fn other_node(arena: &LGraphArena, e: LEdgeId, n: LNodeId) -> LNodeId {
    let s = arena.edge_source_node(e).unwrap();
    let t = arena.edge_target_node(e).unwrap();
    if s == n {
        t
    } else {
        s
    }
}

// ---- balanced median -------------------------------------------------

fn create_balanced_layout(
    arena: &LGraphArena,
    graph: LGraphId,
    layouts: &[Layout],
    node_count: usize,
    dummy: LNodeId,
) -> Option<Layout> {
    let no = layouts.len();
    let mut balanced = Layout::new(node_count, None, None, dummy);
    let mut width = vec![0.0; no];
    let mut min = vec![f64::INFINITY; no];
    let mut max = vec![f64::NEG_INFINITY; no];
    let mut min_width_layout = 0;
    for i in 0..no {
        width[i] = layout_size(arena, graph, &layouts[i]);
        if width[min_width_layout] > width[i] {
            min_width_layout = i;
        }
        for l in 0..arena.graphs[graph.0].layers.len() {
            for &n in &arena.graphs[graph.0].layers[l].nodes {
                let id = gid(arena, n);
                let node_pos_y = layouts[i].y[id].unwrap() + layouts[i].inner_shift[id];
                min[i] = min[i].min(node_pos_y);
                max[i] = max[i].max(node_pos_y + size_y(arena, n));
            }
        }
    }
    let mut shift = vec![0.0; no];
    for i in 0..no {
        shift[i] = if layouts[i].vdir == Some(VDir::Down) {
            min[min_width_layout] - min[i]
        } else {
            max[min_width_layout] - max[i]
        };
    }
    for l in 0..arena.graphs[graph.0].layers.len() {
        for &node in &arena.graphs[graph.0].layers[l].nodes {
            let id = gid(arena, node);
            let mut calc = [0.0f64; 4];
            for i in 0..no {
                calc[i] = layouts[i].y[id].unwrap() + layouts[i].inner_shift[id] + shift[i];
            }
            calc[..no].sort_by(|a, b| a.total_cmp(b));
            balanced.y[id] = Some((calc[1] + calc[2]) / 2.0);
            balanced.inner_shift[id] = 0.0;
        }
    }
    Some(balanced)
}
