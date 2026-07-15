//! Port of ELK `layered` phase 4 (`org.eclipse.elk.alg.layered.p4nodes`):
//! node placement (the in-layer/perpendicular coordinate). EPL-2.0.
//!
//! Currently ported: the placement *input* preparation — the parts of
//! `LabelAndNodeSizeProcessor` (port distribution on node borders) and
//! `InnermostNodeMarginCalculator` (margins) that the Brandes-Köpf placer
//! consumes, plus the direction transpose ELK applies so the whole
//! layout runs in its normalized rightward orientation.

pub mod bk;

use super::graph::{LGraphArena, LGraphId, NodeType};
use super::options::{Direction, PortSide};

/// Prepare the graph for node placement (phase 4), matching the state
/// ELK's pre-P4 intermediate processors leave:
///
/// - **Direction transpose**: ELK lays out rightward internally, so for
///   a vertical user direction (UP/DOWN) the in-layer axis is horizontal
///   — node sizes are transposed so `size.y` is the in-layer extent
///   (the node's output width).
/// - **Port distribution** (`LabelAndNodeSizeProcessor`): a node whose
///   port order is not FIXED_POS gets each side's ports spread evenly
///   along its in-layer extent, `pos.y = size.y·(i+1)/(k+1)` in port
///   order; anchors stay 0. FIXED_POS `LONG_EDGE` dummies keep the port
///   positions the splitter gave them (0).
/// - **Node label placement** (`LabelAndNodeSizeProcessor`): the one
///   placement occurring in scope — `OUTSIDE V_BOTTOM H_CENTER` — puts
///   the label after the node on the layer axis (`labelNode` spacing)
///   and centered on the in-layer axis, in the internal frame with the
///   label's extents transposed.
/// - **Margins** (`InnermostNodeMarginCalculator`): the bounding box of
///   the node, its placed labels and its port boxes; margins are the
///   box's overhang on each side. Flat inputs have no labels and no
///   out-of-bounds ports, so their margins stay 0; compound parents get
///   port overhang (`PORT_BORDER_OFFSET`) and label nodes their label
///   extents.
pub fn prepare_placement(arena: &mut LGraphArena, graph: LGraphId) {
    let direction = arena.graphs[graph.0].props.direction;
    let transpose = matches!(direction, Direction::Up | Direction::Down);

    let layer_count = arena.graphs[graph.0].layers.len();
    for li in 0..layer_count {
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            // Only real (imported) nodes carry output-oriented sizes;
            // LONG_EDGE dummies are created by the splitter already in the
            // internal orientation, so they are not transposed. Neither are
            // compound nodes: their size was set by the child graph's
            // `HierarchicalNodeResizingProcessor` in the internal frame.
            if transpose
                && arena.nodes[node.0].node_type == NodeType::Normal
                && arena.nodes[node.0].nested_graph.is_none()
            {
                let s = arena.nodes[node.0].size;
                arena.nodes[node.0].size = super::math::KVector::new(s.y, s.x);
            }
            if arena.nodes[node.0].props.port_constraints
                == super::options::PortConstraints::FixedPos
            {
                continue; // dummies keep the splitter's port positions
            }
            let size_y = arena.nodes[node.0].size.y;
            for side in [PortSide::East, PortSide::West] {
                let side_ports: Vec<_> = arena.nodes[node.0]
                    .ports
                    .iter()
                    .copied()
                    .filter(|&p| arena.ports[p.0].side == side)
                    .collect();
                let k = side_ports.len();
                // Ports sit on the node border along the layer axis: EAST
                // ports at the right edge (`size.x`), WEST ports at the left
                // (0). The router only reads `y`, but the edge export needs
                // the correct `x` so a source anchor lands on the node's exit
                // edge and a target anchor on its entry edge.
                let side_x = if side == PortSide::East { arena.nodes[node.0].size.x } else { 0.0 };
                for (i, p) in side_ports.into_iter().enumerate() {
                    // EAST ports ascend (top→bottom); WEST ports descend
                    // (bottom→top) — the clockwise placement ELK produces.
                    let num = if side == PortSide::East { i + 1 } else { k - i };
                    arena.ports[p.0].position.x = side_x;
                    arena.ports[p.0].position.y = size_y * num as f64 / (k as f64 + 1.0);
                    arena.ports[p.0].anchor.y = 0.0;
                }
            }
        }
    }

    // Node label placement + margins, after every port has its position.
    let label_node_spacing = arena.graphs[graph.0].props.spacing.label_node;
    for li in 0..layer_count {
        for node in arena.graphs[graph.0].layers[li].nodes.clone() {
            place_node_labels(arena, node, label_node_spacing);
            calculate_margins(arena, node);
        }
    }
}

/// The `OUTSIDE V_BOTTOM H_CENTER` branch of ELK's node label placement,
/// in the internal rightward frame: the label box (extents transposed)
/// sits `labelNode` spacing after the node on the layer axis, centered
/// on the in-layer axis.
fn place_node_labels(arena: &mut LGraphArena, node: super::graph::LNodeId, spacing: f64) {
    for label in arena.nodes[node.0].labels.clone() {
        let placement = arena.labels[label.0].props.node_placement.clone();
        let Some(placement) = placement else { continue };
        assert_eq!(
            placement, "OUTSIDE V_BOTTOM H_CENTER",
            "only OUTSIDE V_BOTTOM H_CENTER node labels are in the ported scope"
        );
        // Transposed label extents: internal x = output height, internal
        // y = output width.
        let (out_w, out_h) = (arena.labels[label.0].size.x, arena.labels[label.0].size.y);
        let node_size = arena.nodes[node.0].size;
        arena.labels[label.0].position.x = node_size.x + spacing;
        arena.labels[label.0].position.y = (node_size.y - out_w) / 2.0;
        let _ = out_h;
    }
}

/// Java `InnermostNodeMarginCalculator` → `NodeMarginCalculator.processNode`:
/// margins = overhang of (node ∪ labels ∪ port boxes) beyond the node box.
/// Port labels and edge end labels never occur in scope.
fn calculate_margins(arena: &mut LGraphArena, node: super::graph::LNodeId) {
    let size = arena.nodes[node.0].size;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0.0f64, 0.0f64, size.x, size.y);

    for &label in &arena.nodes[node.0].labels {
        if arena.labels[label.0].props.node_placement.is_none() {
            continue;
        }
        let p = arena.labels[label.0].position;
        // Internal label extents are the transposed output size.
        let (ext_x, ext_y) = (arena.labels[label.0].size.y, arena.labels[label.0].size.x);
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x + ext_x);
        max_y = max_y.max(p.y + ext_y);
    }
    for &port in &arena.nodes[node.0].ports {
        let p = arena.ports[port.0].position;
        let s = arena.ports[port.0].size;
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x + s.x);
        max_y = max_y.max(p.y + s.y);
    }

    arena.nodes[node.0].margin.left = 0.0f64.max(-min_x);
    arena.nodes[node.0].margin.top = 0.0f64.max(-min_y);
    arena.nodes[node.0].margin.right = 0.0f64.max(max_x - size.x);
    arena.nodes[node.0].margin.bottom = 0.0f64.max(max_y - size.y);
}
