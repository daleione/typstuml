//! Port of the layered algorithm's internal graph model:
//! `LGraphElement` / `LShape` / `LGraph` / `Layer` / `LNode` / `LPort`
//! / `LEdge` / `LLabel` from
//! `org.eclipse.elk.alg.layered.graph`.
//!
//! Java's mutable object graph (elements holding references to each
//! other) becomes a single [`LGraphArena`] owning every element, with
//! typed indices for references. Relationship-maintaining setters
//! (`LNode.setLayer`, `LPort.setNode`, `LEdge.setSource/(…)Target`)
//! are arena methods so both sides of each relationship stay in sync,
//! exactly like the Java setters do.

use super::math::{Insets, KVector, KVectorChain};
use super::options::{
    EdgeLabelPlacement, EdgeProps, GraphProps, LabelProps, NodeProps, PortProps, PortSide,
};

macro_rules! arena_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub usize);
    };
}

arena_id!(LGraphId);
arena_id!(LNodeId);
arena_id!(LPortId);
arena_id!(LEdgeId);
arena_id!(LLabelId);

/// Java `LNode.NodeType`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NodeType {
    #[default]
    Normal,
    LongEdge,
    ExternalPort,
    NorthSouthPort,
    Label,
    BreakingPoint,
    Placeholder,
    NonshiftingPlaceholder,
}

/// Java `LGraph`. `layers` only fills after the layering phase; until
/// then nodes live in `layerless_nodes`.
#[derive(Debug, Clone, Default)]
pub struct LGraph {
    /// Scratch field algorithms use freely (Java `LGraphElement.id`).
    pub id: usize,
    pub size: KVector,
    pub padding: Insets,
    pub offset: KVector,
    pub layerless_nodes: Vec<LNodeId>,
    pub layers: Vec<Layer>,
    pub parent_node: Option<LNodeId>,
    pub props: GraphProps,
}

impl LGraph {
    /// Java `LGraph.getActualSize()`.
    pub fn actual_size(&self) -> KVector {
        KVector::new(
            self.size.x + self.padding.left + self.padding.right,
            self.size.y + self.padding.top + self.padding.bottom,
        )
    }
}

/// Java `Layer` (owned by its graph; `nodes` order = in-layer order).
#[derive(Debug, Clone, Default)]
pub struct Layer {
    pub id: usize,
    pub size: KVector,
    pub nodes: Vec<LNodeId>,
}

/// Java `LNode` (an `LShape`: position + size).
#[derive(Debug, Clone, Default)]
pub struct LNode {
    pub id: usize,
    pub position: KVector,
    pub size: KVector,
    pub margin: Insets,
    pub padding: Insets,
    pub graph: LGraphId,
    /// Index into the owning graph's `layers` (None until layered).
    pub layer: Option<usize>,
    pub node_type: NodeType,
    /// Port order is meaningful (clockwise once sides are fixed).
    pub ports: Vec<LPortId>,
    pub labels: Vec<LLabelId>,
    pub nested_graph: Option<LGraphId>,
    pub props: NodeProps,
}

/// Java `LPort`.
#[derive(Debug, Clone, Default)]
pub struct LPort {
    pub id: usize,
    pub position: KVector,
    pub size: KVector,
    pub margin: Insets,
    pub owner: Option<LNodeId>,
    pub side: PortSide,
    pub anchor: KVector,
    pub explicitly_supplied_port_anchor: bool,
    pub labels: Vec<LLabelId>,
    pub incoming_edges: Vec<LEdgeId>,
    pub outgoing_edges: Vec<LEdgeId>,
    pub props: PortProps,
}

/// Java `LEdge`.
#[derive(Debug, Clone, Default)]
pub struct LEdge {
    pub id: usize,
    pub bend_points: KVectorChain,
    pub source: Option<LPortId>,
    pub target: Option<LPortId>,
    pub labels: Vec<LLabelId>,
    pub props: EdgeProps,
}

/// Java `LLabel`.
#[derive(Debug, Clone, Default)]
pub struct LLabel {
    pub id: usize,
    pub position: KVector,
    pub size: KVector,
    pub text: String,
    pub props: LabelProps,
}

/// Owns every element of every graph in one layout run (the whole
/// nested-graph tree shares the arena, since compound processing moves
/// edges and ports across graph boundaries).
#[derive(Debug, Clone, Default)]
pub struct LGraphArena {
    pub graphs: Vec<LGraph>,
    pub nodes: Vec<LNode>,
    pub ports: Vec<LPort>,
    pub edges: Vec<LEdge>,
    pub labels: Vec<LLabel>,
}

impl LGraphArena {
    pub fn new_graph(&mut self) -> LGraphId {
        self.graphs.push(LGraph::default());
        LGraphId(self.graphs.len() - 1)
    }

    /// Java `new LNode(graph)` + `graph.getLayerlessNodes().add(node)`
    /// (the two always travel together at creation time).
    pub fn new_node(&mut self, graph: LGraphId) -> LNodeId {
        self.nodes.push(LNode { graph, ..LNode::default() });
        let id = LNodeId(self.nodes.len() - 1);
        self.graphs[graph.0].layerless_nodes.push(id);
        id
    }

    pub fn new_port(&mut self, node: LNodeId) -> LPortId {
        self.ports.push(LPort::default());
        let id = LPortId(self.ports.len() - 1);
        self.port_set_node(id, Some(node));
        id
    }

    pub fn new_edge(&mut self) -> LEdgeId {
        self.edges.push(LEdge::default());
        LEdgeId(self.edges.len() - 1)
    }

    pub fn new_label(&mut self, text: impl Into<String>) -> LLabelId {
        self.labels.push(LLabel { text: text.into(), ..LLabel::default() });
        LLabelId(self.labels.len() - 1)
    }

    // ------------------------------------------------------------------
    // Relationship-maintaining setters
    // ------------------------------------------------------------------

    /// Java `LPort.setNode` — unhooks from the old owner's port list,
    /// hooks into the new one's.
    pub fn port_set_node(&mut self, port: LPortId, node: Option<LNodeId>) {
        if let Some(old) = self.ports[port.0].owner {
            self.nodes[old.0].ports.retain(|&p| p != port);
        }
        self.ports[port.0].owner = node;
        if let Some(new) = node {
            self.nodes[new.0].ports.push(port);
        }
    }

    /// Java `LPort.setSide` — also re-derives the default anchor
    /// unless one was explicitly supplied.
    pub fn port_set_side(&mut self, port: LPortId, side: PortSide) {
        let p = &mut self.ports[port.0];
        p.side = side;
        if !p.explicitly_supplied_port_anchor {
            match side {
                PortSide::North => p.anchor = KVector::new(p.size.x / 2.0, 0.0),
                PortSide::East => p.anchor = KVector::new(p.size.x, p.size.y / 2.0),
                PortSide::South => p.anchor = KVector::new(p.size.x / 2.0, p.size.y),
                PortSide::West => p.anchor = KVector::new(0.0, p.size.y / 2.0),
                PortSide::Undefined => {}
            }
        }
    }

    /// Java `LPort.getAbsoluteAnchor()`: owner position + port
    /// position + anchor.
    pub fn port_absolute_anchor(&self, port: LPortId) -> KVector {
        let p = &self.ports[port.0];
        let owner = p.owner.expect("port has no owner");
        self.nodes[owner.0].position.add(p.position).add(p.anchor)
    }

    /// Java `LEdge.setSource` — maintains the ports' outgoing lists.
    pub fn edge_set_source(&mut self, edge: LEdgeId, source: Option<LPortId>) {
        if let Some(old) = self.edges[edge.0].source {
            self.ports[old.0].outgoing_edges.retain(|&e| e != edge);
        }
        self.edges[edge.0].source = source;
        if let Some(new) = source {
            self.ports[new.0].outgoing_edges.push(edge);
        }
    }

    /// Java `LEdge.setTarget`.
    pub fn edge_set_target(&mut self, edge: LEdgeId, target: Option<LPortId>) {
        if let Some(old) = self.edges[edge.0].target {
            self.ports[old.0].incoming_edges.retain(|&e| e != edge);
        }
        self.edges[edge.0].target = target;
        if let Some(new) = target {
            self.ports[new.0].incoming_edges.push(edge);
        }
    }

    /// Java `LEdge.setTargetAndInsertAtIndex` — like `setTarget` but
    /// controls the position in the target port's incoming list
    /// (matters for FIXED_ORDER crossings).
    pub fn edge_set_target_at_index(&mut self, edge: LEdgeId, target: LPortId, index: usize) {
        if let Some(old) = self.edges[edge.0].target {
            self.ports[old.0].incoming_edges.retain(|&e| e != edge);
        }
        self.edges[edge.0].target = Some(target);
        self.ports[target.0].incoming_edges.insert(index, edge);
    }

    /// Java `LEdge.reverse(layeredGraph, adaptPorts)`. The collector-
    /// port branch (`INPUT_COLLECT`/`OUTPUT_COLLECT`, hypernode
    /// support) is outside the ported scope — draw-uml never sets it —
    /// and deliberately panics rather than silently diverging.
    pub fn edge_reverse(&mut self, edge: LEdgeId, adapt_ports: bool) {
        let old_source = self.edges[edge.0].source;
        let old_target = self.edges[edge.0].target;
        self.edge_set_source(edge, None);
        self.edge_set_target(edge, None);
        if adapt_ports {
            let collect = old_target.is_some_and(|p| self.ports[p.0].props.input_collect)
                || old_source.is_some_and(|p| self.ports[p.0].props.output_collect);
            assert!(!collect, "collector ports (hypernodes) are outside the ported scope");
        }
        self.edge_set_source(edge, old_target);
        self.edge_set_target(edge, old_source);

        let labels = self.edges[edge.0].labels.clone();
        for label in labels {
            let placement = &mut self.labels[label.0].props.placement;
            *placement = match *placement {
                EdgeLabelPlacement::Tail => EdgeLabelPlacement::Head,
                EdgeLabelPlacement::Head => EdgeLabelPlacement::Tail,
                EdgeLabelPlacement::Center => EdgeLabelPlacement::Center,
            };
        }

        let e = &mut self.edges[edge.0];
        e.props.reversed = !e.props.reversed;
        e.bend_points.reverse();
    }

    /// Java `LNode.setLayer(layer)` — unhooks from the previous layer's
    /// node list, hooks into the new one (append). Layers are indices
    /// into the owning graph's `layers`; [`Self::insert_layer`] keeps
    /// the stored indices consistent when the list shifts.
    pub fn node_set_layer(&mut self, graph: LGraphId, node: LNodeId, layer_idx: Option<usize>) {
        if let Some(old) = self.nodes[node.0].layer {
            self.graphs[graph.0].layers[old].nodes.retain(|&n| n != node);
        }
        self.nodes[node.0].layer = layer_idx;
        if let Some(new) = layer_idx {
            self.graphs[graph.0].layers[new].nodes.push(node);
        }
    }

    /// Java `LNode.setLayer(index, layer)` — like [`Self::node_set_layer`]
    /// but inserts at a specific position in the layer's node list (the
    /// label dummy switcher swaps nodes without disturbing the crossing-
    /// minimized in-layer order).
    pub fn node_set_layer_at_index(
        &mut self,
        graph: LGraphId,
        node: LNodeId,
        layer_idx: usize,
        index: usize,
    ) {
        if let Some(old) = self.nodes[node.0].layer {
            self.graphs[graph.0].layers[old].nodes.retain(|&n| n != node);
        }
        self.nodes[node.0].layer = Some(layer_idx);
        self.graphs[graph.0].layers[layer_idx].nodes.insert(index, node);
    }

    /// Insert an empty layer at `at`, shifting the stored layer index
    /// of every node in layers `at..` (Java layers are stable object
    /// pointers; index-based storage must compensate).
    pub fn insert_layer(&mut self, graph: LGraphId, at: usize) {
        self.graphs[graph.0].layers.insert(at, Layer::default());
        let shifted: Vec<LNodeId> = self.graphs[graph.0].layers[at + 1..]
            .iter()
            .flat_map(|l| l.nodes.iter().copied())
            .collect();
        for n in shifted {
            if let Some(l) = self.nodes[n.0].layer {
                if l >= at {
                    self.nodes[n.0].layer = Some(l + 1);
                }
            }
        }
    }

    /// Remove empty layers (Java: iterator removal at the end of
    /// several processors), fixing stored indices.
    pub fn remove_empty_layers(&mut self, graph: LGraphId) {
        let keep: Vec<bool> =
            self.graphs[graph.0].layers.iter().map(|l| !l.nodes.is_empty()).collect();
        let mut new_index = vec![0usize; keep.len()];
        let mut next = 0usize;
        for (i, &k) in keep.iter().enumerate() {
            new_index[i] = next;
            if k {
                next += 1;
            }
        }
        let mut i = 0;
        self.graphs[graph.0].layers.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        for n in 0..self.nodes.len() {
            if self.nodes[n].graph == graph {
                if let Some(l) = self.nodes[n].layer {
                    self.nodes[n].layer = Some(new_index[l]);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Read helpers
    // ------------------------------------------------------------------

    /// Java `LEdge.isSelfLoop()`.
    pub fn edge_is_self_loop(&self, edge: LEdgeId) -> bool {
        let e = &self.edges[edge.0];
        match (e.source, e.target) {
            (Some(s), Some(t)) => {
                let sn = self.ports[s.0].owner;
                sn.is_some() && sn == self.ports[t.0].owner
            }
            _ => false,
        }
    }

    /// Owner node of an edge endpoint.
    pub fn edge_source_node(&self, edge: LEdgeId) -> Option<LNodeId> {
        self.ports[self.edges[edge.0].source?.0].owner
    }

    pub fn edge_target_node(&self, edge: LEdgeId) -> Option<LNodeId> {
        self.ports[self.edges[edge.0].target?.0].owner
    }

    /// All edges incident to any port of `node`, outgoing then
    /// incoming per port, port order preserved (Java iterates
    /// `port.getConnectedEdges()` per port the same way).
    pub fn node_connected_edges(&self, node: LNodeId) -> Vec<LEdgeId> {
        let mut out = Vec::new();
        for &p in &self.nodes[node.0].ports {
            out.extend(self.ports[p.0].incoming_edges.iter().copied());
            out.extend(self.ports[p.0].outgoing_edges.iter().copied());
        }
        out
    }

    /// Outgoing edges across all ports of `node`.
    pub fn node_outgoing_edges(&self, node: LNodeId) -> Vec<LEdgeId> {
        let mut out = Vec::new();
        for &p in &self.nodes[node.0].ports {
            out.extend(self.ports[p.0].outgoing_edges.iter().copied());
        }
        out
    }

    pub fn node_incoming_edges(&self, node: LNodeId) -> Vec<LEdgeId> {
        let mut out = Vec::new();
        for &p in &self.nodes[node.0].ports {
            out.extend(self.ports[p.0].incoming_edges.iter().copied());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_edge(arena: &mut LGraphArena, g: LGraphId) -> (LNodeId, LNodeId, LEdgeId) {
        let a = arena.new_node(g);
        let b = arena.new_node(g);
        let pa = arena.new_port(a);
        let pb = arena.new_port(b);
        let e = arena.new_edge();
        arena.edge_set_source(e, Some(pa));
        arena.edge_set_target(e, Some(pb));
        (a, b, e)
    }

    #[test]
    fn edge_reverse_swaps_endpoints_and_marks_reversed() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let (a, b, e) = simple_edge(&mut arena, g);

        assert_eq!(arena.edge_source_node(e), Some(a));
        assert_eq!(arena.edge_target_node(e), Some(b));
        assert!(!arena.edges[e.0].props.reversed);

        arena.edge_reverse(e, true);
        assert_eq!(arena.edge_source_node(e), Some(b));
        assert_eq!(arena.edge_target_node(e), Some(a));
        assert!(arena.edges[e.0].props.reversed);
        // Port edge lists stay consistent.
        let pa = arena.nodes[a.0].ports[0];
        let pb = arena.nodes[b.0].ports[0];
        assert_eq!(arena.ports[pa.0].incoming_edges, vec![e]);
        assert!(arena.ports[pa.0].outgoing_edges.is_empty());
        assert_eq!(arena.ports[pb.0].outgoing_edges, vec![e]);
        assert!(arena.ports[pb.0].incoming_edges.is_empty());

        // Reversing again restores the original direction and flag.
        arena.edge_reverse(e, true);
        assert_eq!(arena.edge_source_node(e), Some(a));
        assert!(!arena.edges[e.0].props.reversed);
    }

    #[test]
    fn port_side_derives_anchor() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let n = arena.new_node(g);
        let p = arena.new_port(n);
        arena.ports[p.0].size = KVector::new(10.0, 6.0);
        arena.port_set_side(p, PortSide::East);
        assert_eq!(arena.ports[p.0].anchor, KVector::new(10.0, 3.0));
        arena.port_set_side(p, PortSide::South);
        assert_eq!(arena.ports[p.0].anchor, KVector::new(5.0, 6.0));

        // Explicit anchors are never overwritten.
        arena.ports[p.0].explicitly_supplied_port_anchor = true;
        arena.ports[p.0].anchor = KVector::new(1.0, 1.0);
        arena.port_set_side(p, PortSide::West);
        assert_eq!(arena.ports[p.0].anchor, KVector::new(1.0, 1.0));
    }

    #[test]
    fn self_loop_detection() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let n = arena.new_node(g);
        let p1 = arena.new_port(n);
        let p2 = arena.new_port(n);
        let e = arena.new_edge();
        arena.edge_set_source(e, Some(p1));
        arena.edge_set_target(e, Some(p2));
        assert!(arena.edge_is_self_loop(e));
    }

    #[test]
    fn absolute_anchor_composes_owner_port_anchor() {
        let mut arena = LGraphArena::default();
        let g = arena.new_graph();
        let n = arena.new_node(g);
        arena.nodes[n.0].position = KVector::new(100.0, 200.0);
        let p = arena.new_port(n);
        arena.ports[p.0].position = KVector::new(30.0, 0.0);
        arena.ports[p.0].size = KVector::new(8.0, 8.0);
        arena.port_set_side(p, PortSide::North);
        assert_eq!(arena.port_absolute_anchor(p), KVector::new(134.0, 200.0));
    }
}
