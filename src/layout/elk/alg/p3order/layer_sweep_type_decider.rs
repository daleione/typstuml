//! Port of `org.eclipse.elk.alg.layered.p3order.LayerSweepTypeDecider`
//! (EPL-2.0, see `../LICENSE.md`) — decides, per graph, whether the
//! crossing minimizer sweeps it **bottom-up** (independently, then feeds
//! its parent's port order) or lets the parent sweep **into** it
//! (cross-hierarchy).
//!
//! `useBottomUp()` = `normalized >= hierarchicalSweepiness` (default
//! **0.1**, not −1), where
//! `normalized = (pathsToRandom − pathsToHierarchical) / allPaths ∈
//! [−1, 1]` (`+∞` if no paths). Higher ⇒ favour cross-hierarchy.
//!
//! **Orientation note**: ELK runs everything in its normalized rightward
//! frame (WEST = input, EAST = output). This port skips ELK's import
//! rotation, so real-node ports are already WEST/EAST but group-node and
//! external-port-dummy ports keep the DOWN-direction NORTH/SOUTH the
//! compound preprocessor gave them. We therefore read a *normalized*
//! side (NORTH→WEST, SOUTH→EAST) everywhere the decider inspects sides,
//! so group ports line up with the rightward convention the algorithm
//! assumes.

use super::super::graph::{LGraphArena, LGraphId, LNodeId, NodeType};
use super::super::options::PortSide;

const SWEEPINESS: f64 = 0.1;

/// Normalize a port side into the rightward frame: NORTH acts as WEST
/// (input side), SOUTH as EAST (output side); WEST/EAST pass through.
fn rightward(side: PortSide) -> PortSide {
    match side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    }
}

#[derive(Clone, Copy, Default)]
struct NodeInfo {
    connected_edges: i32,
    hierarchical_influence: i32,
    random_influence: i32,
}

/// Java `LayerSweepTypeDecider.useBottomUp()`. `graph` must be layered
/// with port sides assigned. `bary_deterministic` is `false` for the
/// default BARYCENTER heuristic (so the path-ratio branch runs).
pub fn use_bottom_up(arena: &LGraphArena, graph: LGraphId, bary_deterministic: bool) -> bool {
    let boundary = SWEEPINESS;
    // gates
    if boundary < -1.0 {
        return true; // bottomUpForced
    }
    let parent = arena.graphs[graph.0].parent_node;
    let Some(parent) = parent else {
        return true; // rootNode
    };
    if arena.nodes[parent.0].props.port_constraints.is_order_fixed() {
        return true; // fixedPortOrder
    }
    if port_side_count(arena, parent, PortSide::East) < 2
        && port_side_count(arena, parent, PortSide::West) < 2
    {
        return true; // fewerThanTwoInOutEdges
    }
    if bary_deterministic {
        return false;
    }

    // Path-ratio. NodeInfo indexed [layer][pos]; node.id must be the
    // node's position in its layer (set by the sweep's initialize).
    let layers = &arena.graphs[graph.0].layers;
    let mut info: Vec<Vec<NodeInfo>> =
        layers.iter().map(|l| vec![NodeInfo::default(); l.nodes.len()]).collect();

    let mut paths_to_random: i64 = 0;
    let mut paths_to_hierarchical: i64 = 0;

    // No NORTH_SOUTH_PORT dummies occur in scope, so the north/south
    // deferral list stays empty.
    for li in 0..layers.len() {
        for ni in 0..layers[li].nodes.len() {
            let node = layers[li].nodes[ni];
            let (li_n, ni_n) = node_index(arena, node);
            if arena.nodes[node.0].node_type == NodeType::ExternalPort {
                info[li_n][ni_n].hierarchical_influence = 1;
                if is_eastern_dummy(arena, node) {
                    paths_to_hierarchical += info[li_n][ni_n].connected_edges as i64;
                }
            } else if has_no_side_ports(arena, node, PortSide::West) {
                info[li_n][ni_n].random_influence = 1;
            } else if has_no_side_ports(arena, node, PortSide::East) {
                paths_to_random += info[li_n][ni_n].connected_edges as i64;
            }

            let cur = info[li_n][ni_n];
            for edge in arena.node_outgoing_edges(node) {
                paths_to_random += cur.random_influence as i64;
                paths_to_hierarchical += cur.hierarchical_influence as i64;
                let target = arena.edge_target_node(edge).unwrap();
                let (lt, nt) = node_index(arena, target);
                info[lt][nt].hierarchical_influence += cur.hierarchical_influence;
                info[lt][nt].random_influence += cur.random_influence;
                info[lt][nt].connected_edges += cur.connected_edges;
                info[lt][nt].connected_edges += 1;
            }
        }
    }

    let all_paths = paths_to_random + paths_to_hierarchical;
    if all_paths == 0 {
        return true; // normalized = +∞ >= boundary
    }
    let normalized = (paths_to_random - paths_to_hierarchical) as f64 / all_paths as f64;
    normalized >= boundary
}

fn node_index(arena: &LGraphArena, node: LNodeId) -> (usize, usize) {
    (arena.nodes[node.0].layer.unwrap(), arena.nodes[node.0].id)
}

/// Count of `parent`'s ports on the normalized `side`.
fn port_side_count(arena: &LGraphArena, parent: LNodeId, side: PortSide) -> usize {
    arena.nodes[parent.0]
        .ports
        .iter()
        .filter(|&&p| rightward(arena.ports[p.0].side) == side)
        .count()
}

/// Java `hasNoWesternPorts`/`hasNoEasternPorts`: the node has no ports on
/// `side`, or none of them carry a connected edge.
fn has_no_side_ports(arena: &LGraphArena, node: LNodeId, side: PortSide) -> bool {
    let ports: Vec<_> = arena.nodes[node.0]
        .ports
        .iter()
        .copied()
        .filter(|&p| rightward(arena.ports[p.0].side) == side)
        .collect();
    ports.is_empty()
        || !ports.iter().any(|&p| {
            !arena.ports[p.0].incoming_edges.is_empty()
                || !arena.ports[p.0].outgoing_edges.is_empty()
        })
}

/// Java `isEasternDummy`: the external-port dummy's origin (parent) port
/// is on the (normalized) EAST side.
fn is_eastern_dummy(arena: &LGraphArena, dummy: LNodeId) -> bool {
    let Some(origin) = arena.nodes[dummy.0].props.origin_port else {
        return false;
    };
    rightward(arena.ports[origin.0].side) == PortSide::East
}
