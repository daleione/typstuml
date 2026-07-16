//! Port of `org.eclipse.elk.alg.layered.options.Spacings` (the
//! node-type spacing table the BK node placer and compactor query).
//!
//! `getVerticalSpacing(n1, n2)` picks the layout option for the pair of
//! node types from the *vertical* table, then (with no per-node
//! `SPACING_INDIVIDUAL` overrides in scope) returns that graph option's
//! value. The horizontal (between-layers) table is used for the layer
//! axis and is added when E6's layer positioning lands.
//!
//! Only the node-type pairs reachable in draw-uml's flat/architecture
//! scope are wired: NORMAL and LONG_EDGE (real nodes + long-edge
//! dummies). Other pairs (north/south, external ports, labels, breaking
//! points) don't occur here and return an explicit panic if queried, so
//! a scope violation can't silently use a wrong spacing.

use super::graph::NodeType;
use super::options::SpacingProps;

/// The vertical (in-layer) spacing to preserve between two node types.
/// Java `Spacings.getVerticalSpacing(n1, n2)` = `max(s1, s2)` over each
/// node's individual-or-default value; with no individual overrides both
/// resolve to the same graph option, so this is that option's value.
pub fn vertical_spacing(spacing: &SpacingProps, t1: NodeType, t2: NodeType) -> f64 {
    use NodeType::{ExternalPort, Label, LongEdge, Normal};
    match (t1, t2) {
        (Normal, Normal) => spacing.node_node,
        (Normal, LongEdge) | (LongEdge, Normal) => spacing.edge_node,
        (LongEdge, LongEdge) => spacing.edge_edge,
        // Compound boundary layers (ELK's table, same source order):
        (Normal, ExternalPort) | (ExternalPort, Normal) => spacing.edge_node,
        (LongEdge, ExternalPort) | (ExternalPort, LongEdge) => spacing.edge_edge,
        (ExternalPort, ExternalPort) => spacing.port_port,
        // Center-edge-label dummies:
        (Normal, Label) | (Label, Normal) => spacing.node_node,
        (LongEdge, Label) | (Label, LongEdge) => spacing.edge_node,
        (Label, Label) => spacing.edge_edge,
        (ExternalPort, Label) | (Label, ExternalPort) => spacing.label_port_vertical,
        _ => panic!(
            "vertical spacing for node types {t1:?}/{t2:?} is outside the ported scope"
        ),
    }
}

/// The horizontal (between-layers) spacing to preserve between two node
/// types — Java `Spacings.getHorizontalSpacing(n1, n2)`. External-port
/// pairs are unreachable: the compactor's spacing handler returns 0 for
/// them before consulting the table.
pub fn horizontal_spacing(spacing: &SpacingProps, t1: NodeType, t2: NodeType) -> f64 {
    use NodeType::{ExternalPort, Label, LongEdge, Normal};
    match (t1, t2) {
        (Normal, Normal) => spacing.node_node_between_layers,
        (Normal, LongEdge) | (LongEdge, Normal) => spacing.edge_node_between_layers,
        (LongEdge, LongEdge) => spacing.edge_edge_between_layers,
        // Center-edge-label dummies (ELK's table: LABEL×LABEL uses the
        // plain edgeEdge option on the horizontal axis too):
        (Normal, Label) | (Label, Normal) => spacing.node_node_between_layers,
        (LongEdge, Label) | (Label, LongEdge) => spacing.edge_node_between_layers,
        (Label, Label) => spacing.edge_edge,
        (ExternalPort, Label) | (Label, ExternalPort) => spacing.label_port_horizontal,
        _ => panic!(
            "horizontal spacing for node types {t1:?}/{t2:?} is outside the ported scope"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_spacing_table_matches_elk() {
        // draw-uml benchmark: nodeNode=10, edgeNode/edgeEdge at ELK
        // defaults (10). Table lookups follow ELK's precalculated
        // node-type spacing map.
        let s = SpacingProps { node_node: 10.0, edge_node: 8.0, edge_edge: 6.0, ..Default::default() };
        assert_eq!(vertical_spacing(&s, NodeType::Normal, NodeType::Normal), 10.0);
        assert_eq!(vertical_spacing(&s, NodeType::Normal, NodeType::LongEdge), 8.0);
        assert_eq!(vertical_spacing(&s, NodeType::LongEdge, NodeType::Normal), 8.0);
        assert_eq!(vertical_spacing(&s, NodeType::LongEdge, NodeType::LongEdge), 6.0);
    }
}
