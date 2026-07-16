//! Port of
//! `org.eclipse.elk.alg.layered.compound.CompoundGraphPreprocessor`
//! (+ the `LGraphUtil.createExternalPortDummy` helper it drives) for
//! `hierarchyHandling: INCLUDE_CHILDREN` graphs.
//!
//! A cross-hierarchy edge cannot be laid out as one piece — each graph
//! in the nesting tree is laid out on its own — so the preprocessor
//! splits every such edge into per-graph *segments*: an external-port
//! dummy node inside each crossed nested graph, a fresh port on each
//! crossed compound node, and one dummy edge per traversed graph. The
//! original edges are disconnected and remembered in the returned
//! cross-hierarchy map; `CompoundGraphPostprocessor` (ported later,
//! with the exporter) reassembles them from the routed segments.
//!
//! Scope notes (see docs/elk-port-plan.md §2): inside self-loops and
//! port labels never occur in draw-uml inputs; both paths assert
//! instead of half-running. Java's `CROSS_HIERARCHY_MAP` graph
//! property becomes this module's return value.

use std::collections::HashMap;

use super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId, LPortId, NodeType};
use super::hierarchical;
use super::math::KVector;
use super::options::{
    Direction, EdgeConstraint, InLayerConstraint, LayerConstraint, PortConstraints, PortSide,
    PortType,
};

/// Java `CrossHierarchyEdge`: one segment of a split cross-hierarchy
/// edge — the dummy edge standing in for the original within `graph`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossHierarchyEdge {
    pub edge: LEdgeId,
    pub graph: LGraphId,
    pub port_type: PortType,
}

/// Java `CompoundGraphPreprocessor.process(graph)`. Returns the
/// cross-hierarchy map: original edge → its segments in creation
/// order.
pub fn preprocess(
    arena: &mut LGraphArena,
    graph: LGraphId,
) -> HashMap<LEdgeId, Vec<CrossHierarchyEdge>> {
    let mut pre = Preprocessor {
        arena,
        cross_hierarchy_map: HashMap::new(),
        dummy_node_map: HashMap::new(),
    };
    pre.transform_hierarchy_edges(graph, None);
    pre.move_labels_and_remove_original_edges(graph);
    pre.set_sides_of_ports_to_sides_of_dummy_nodes();
    pre.cross_hierarchy_map
}

/// Java's private `ExternalPort` helper class.
struct ExternalPort {
    orig_edges: Vec<LEdgeId>,
    new_edge: LEdgeId,
    dummy_node: LNodeId,
    /// The port on the *parent (compound) node* this external port
    /// belongs to.
    dummy_port: LPortId,
    port_type: PortType,
    exported: bool,
}

struct Preprocessor<'a> {
    arena: &'a mut LGraphArena,
    cross_hierarchy_map: HashMap<LEdgeId, Vec<CrossHierarchyEdge>>,
    /// Java `dummyNodeMap`: parent-node port → external-port dummy.
    dummy_node_map: HashMap<LPortId, LNodeId>,
}

impl Preprocessor<'_> {
    /// Java `transformHierarchyEdges(graph, parentNode)`.
    fn transform_hierarchy_edges(
        &mut self,
        graph: LGraphId,
        parent_node: Option<LNodeId>,
    ) -> Vec<ExternalPort> {
        // Recurse into nested graphs first.
        let mut contained_external_ports: Vec<ExternalPort> = Vec::new();
        let nodes: Vec<LNodeId> = self.arena.graphs[graph.0].layerless_nodes.clone();
        for node in nodes {
            let Some(nested_graph) = self.arena.nodes[node.0].nested_graph else {
                continue;
            };
            let child_ports = self.transform_hierarchy_edges(nested_graph, Some(node));
            contained_external_ports.extend(child_ports);
            // Java: processInsideSelfLoops — INSIDE_SELF_LOOPS_ACTIVATE
            // is never set in scope, so this is the early-return path.

            if self.arena.graphs[nested_graph.0].props.graph_properties.external_ports {
                // Sync any pre-existing ports of the compound node with
                // external-port dummies. In scope every port of a
                // compound node is created by this very preprocessor
                // (and thus already mapped); a port with edges attached
                // directly to the compound node would land here.
                for port in self.arena.nodes[node.0].ports.clone() {
                    assert!(
                        self.dummy_node_map.contains_key(&port),
                        "edges attached directly to a compound node are outside the ported scope"
                    );
                    assert!(
                        self.arena.ports[port.0].labels.is_empty(),
                        "port labels are outside the ported scope"
                    );
                }
            }
        }

        let mut exported_external_ports: Vec<ExternalPort> = Vec::new();
        self.process_inner_hierarchical_edge_segments(
            graph,
            parent_node,
            &mut contained_external_ports,
            &mut exported_external_ports,
        );
        if let Some(parent) = parent_node {
            self.process_outer_hierarchical_edge_segments(
                graph,
                parent,
                &mut exported_external_ports,
            );
        }
        exported_external_ports
    }

    /// Java `moveLabelsAndRemoveOriginalEdges`: move every label of a
    /// cross-hierarchy edge onto one of its dummy segments — CENTER
    /// labels to the shallowest segment, TAIL to the first, HEAD to the
    /// last — then disconnect the original edge. The receiving segment's
    /// graph gets both END_LABELS and CENTER_LABELS (bug-for-bug: Java
    /// adds both regardless of the placement).
    fn move_labels_and_remove_original_edges(&mut self, top: LGraphId) {
        let orig_edges: Vec<LEdgeId> = self.cross_hierarchy_map.keys().copied().collect();
        for orig_edge in orig_edges {
            if !self.arena.edges[orig_edge.0].labels.is_empty() {
                let mut segments: Vec<CrossHierarchyEdge> =
                    self.cross_hierarchy_map[&orig_edge].clone();
                segments.sort_by(|a, b| cross_hierarchy_edge_cmp(self.arena, a, b, top));

                let labels = self.arena.edges[orig_edge.0].labels.clone();
                let mut kept: Vec<super::graph::LLabelId> = Vec::new();
                for label in labels {
                    let target_index: Option<usize> =
                        match self.arena.labels[label.0].props.placement {
                            super::options::EdgeLabelPlacement::Head => Some(segments.len() - 1),
                            super::options::EdgeLabelPlacement::Center => {
                                shallowest_edge_segment(&segments)
                            }
                            super::options::EdgeLabelPlacement::Tail => Some(0),
                        };
                    match target_index {
                        Some(idx) => {
                            let target_edge = segments[idx].edge;
                            self.arena.edges[target_edge.0].labels.push(label);
                            let src_node = self.arena.edge_source_node(target_edge).unwrap();
                            let src_graph = self.arena.nodes[src_node.0].graph;
                            let gp =
                                &mut self.arena.graphs[src_graph.0].props.graph_properties;
                            gp.end_labels = true;
                            gp.center_labels = true;
                            self.arena.labels[label.0].props.original_label_edge =
                                Some(orig_edge);
                        }
                        None => kept.push(label),
                    }
                }
                self.arena.edges[orig_edge.0].labels = kept;
            }
            self.arena.edge_set_source(orig_edge, None);
            self.arena.edge_set_target(orig_edge, None);
        }
    }

    /// Java `setSidesOfPortsToSidesOfDummyNodes`.
    fn set_sides_of_ports_to_sides_of_dummy_nodes(&mut self) {
        let entries: Vec<(LPortId, LNodeId)> =
            self.dummy_node_map.iter().map(|(&p, &n)| (p, n)).collect();
        for (external_port, dummy_node) in entries {
            self.arena.nodes[dummy_node.0].props.origin_port = Some(external_port);
            self.arena.ports[external_port.0].props.port_dummy = Some(dummy_node);
            self.arena.ports[external_port.0].props.inside_connections = true;
            let side = self.arena.nodes[dummy_node.0].props.ext_port_side;
            self.arena.port_set_side(external_port, side);
            let owner = self.arena.ports[external_port.0].owner.unwrap();
            self.arena.nodes[owner.0].props.port_constraints = PortConstraints::FixedSide;
            let owner_graph = self.arena.nodes[owner.0].graph;
            self.arena.graphs[owner_graph.0].props.graph_properties.non_free_ports = true;
        }
    }

    /// Java `processInnerHierarchicalEdgeSegments`.
    fn process_inner_hierarchical_edge_segments(
        &mut self,
        graph: LGraphId,
        parent_node: Option<LNodeId>,
        contained_external_ports: &mut Vec<ExternalPort>,
        exported_external_ports: &mut Vec<ExternalPort>,
    ) {
        let mut created_external_ports: Vec<ExternalPort> = Vec::new();
        for i in 0..contained_external_ports.len() {
            // `currentExternalPort` merges several original edges into
            // one hierarchical segment when mergeHierarchyEdges is on.
            let mut current: Option<usize> = None; // index into created_external_ports
            let (port_type, orig_edges, dummy_port) = {
                let ep = &contained_external_ports[i];
                (ep.port_type, ep.orig_edges.clone(), ep.dummy_port)
            };
            for orig_edge in orig_edges {
                if port_type == PortType::Output {
                    let target_port = self.arena.edges[orig_edge.0].target.unwrap();
                    let target_node = self.arena.ports[target_port.0].owner.unwrap();
                    if self.arena.nodes[target_node.0].graph == graph {
                        self.connect_child(graph, port_type, orig_edge, dummy_port, target_port);
                    } else if parent_node
                        .is_none_or(|p| self.is_descendant(target_node, p))
                    {
                        self.connect_siblings(
                            graph,
                            port_type,
                            dummy_port,
                            contained_external_ports,
                            i,
                            orig_edge,
                        );
                    } else {
                        self.introduce_segment(
                            graph,
                            parent_node.unwrap(),
                            orig_edge,
                            dummy_port,
                            PortType::Output,
                            &mut current,
                            &mut created_external_ports,
                        );
                    }
                } else {
                    let source_port = self.arena.edges[orig_edge.0].source.unwrap();
                    let source_node = self.arena.ports[source_port.0].owner.unwrap();
                    if self.arena.nodes[source_node.0].graph == graph {
                        self.connect_child(graph, port_type, orig_edge, source_port, dummy_port);
                    } else if parent_node
                        .is_none_or(|p| self.is_descendant(source_node, p))
                    {
                        // Handled from the output side.
                        continue;
                    } else {
                        self.introduce_segment(
                            graph,
                            parent_node.unwrap(),
                            orig_edge,
                            dummy_port,
                            PortType::Input,
                            &mut current,
                            &mut created_external_ports,
                        );
                    }
                }
            }
        }
        self.commit_created_ports(graph, created_external_ports, exported_external_ports);
    }

    /// Java `connectChild`.
    fn connect_child(
        &mut self,
        graph: LGraphId,
        port_type: PortType,
        orig_edge: LEdgeId,
        source_port: LPortId,
        target_port: LPortId,
    ) {
        let dummy_edge = self.create_dummy_edge(orig_edge);
        self.arena.edge_set_source(dummy_edge, Some(source_port));
        self.arena.edge_set_target(dummy_edge, Some(target_port));
        self.cross_hierarchy_map.entry(orig_edge).or_default().push(CrossHierarchyEdge {
            edge: dummy_edge,
            graph,
            port_type,
        });
    }

    /// Java `connectSiblings`.
    fn connect_siblings(
        &mut self,
        graph: LGraphId,
        output_port_type: PortType,
        output_dummy_port: LPortId,
        contained_external_ports: &[ExternalPort],
        output_index: usize,
        orig_edge: LEdgeId,
    ) {
        let target_external_port = contained_external_ports
            .iter()
            .enumerate()
            .find(|(j, ep)| *j != output_index && ep.orig_edges.contains(&orig_edge))
            .map(|(_, ep)| ep)
            .expect("sibling segment must have a matching input external port");
        assert_eq!(target_external_port.port_type, PortType::Input);
        let target_dummy_port = target_external_port.dummy_port;
        let dummy_edge = self.create_dummy_edge(orig_edge);
        self.arena.edge_set_source(dummy_edge, Some(output_dummy_port));
        self.arena.edge_set_target(dummy_edge, Some(target_dummy_port));
        self.cross_hierarchy_map.entry(orig_edge).or_default().push(CrossHierarchyEdge {
            edge: dummy_edge,
            graph,
            port_type: output_port_type,
        });
    }

    /// Java `processOuterHierarchicalEdgeSegments`.
    fn process_outer_hierarchical_edge_segments(
        &mut self,
        graph: LGraphId,
        parent_node: LNodeId,
        exported_external_ports: &mut Vec<ExternalPort>,
    ) {
        let mut created_external_ports: Vec<ExternalPort> = Vec::new();
        let child_nodes: Vec<LNodeId> = self.arena.graphs[graph.0].layerless_nodes.clone();
        for child_node in child_nodes {
            for child_port in self.arena.nodes[child_node.0].ports.clone() {
                let mut current_output: Option<usize> = None;
                for out_edge in self.arena.ports[child_port.0].outgoing_edges.clone() {
                    let target_node = self.arena.edge_target_node(out_edge).unwrap();
                    if !self.is_descendant(target_node, parent_node) {
                        let source_port = self.arena.edges[out_edge.0].source.unwrap();
                        self.introduce_segment(
                            graph,
                            parent_node,
                            out_edge,
                            source_port,
                            PortType::Output,
                            &mut current_output,
                            &mut created_external_ports,
                        );
                    }
                }
                let mut current_input: Option<usize> = None;
                for in_edge in self.arena.ports[child_port.0].incoming_edges.clone() {
                    let source_node = self.arena.edge_source_node(in_edge).unwrap();
                    if !self.is_descendant(source_node, parent_node) {
                        let target_port = self.arena.edges[in_edge.0].target.unwrap();
                        self.introduce_segment(
                            graph,
                            parent_node,
                            in_edge,
                            target_port,
                            PortType::Input,
                            &mut current_input,
                            &mut created_external_ports,
                        );
                    }
                }
            }
        }
        self.commit_created_ports(graph, created_external_ports, exported_external_ports);
    }

    /// Shared tail of both `process*Segments` methods: adopt created
    /// dummies into the graph and pass exported ports upward.
    fn commit_created_ports(
        &mut self,
        graph: LGraphId,
        created: Vec<ExternalPort>,
        exported: &mut Vec<ExternalPort>,
    ) {
        for external_port in created {
            let nodes = &mut self.arena.graphs[graph.0].layerless_nodes;
            if !nodes.contains(&external_port.dummy_node) {
                nodes.push(external_port.dummy_node);
            }
            if external_port.exported {
                exported.push(external_port);
            }
        }
    }

    /// Java `introduceHierarchicalEdgeSegment`: create (or extend,
    /// under mergeHierarchyEdges) the segment of `orig_edge` that
    /// crosses `parent_node`'s boundary within `graph`. `current`
    /// indexes the merge candidate in `created`.
    #[allow(clippy::too_many_arguments)]
    fn introduce_segment(
        &mut self,
        graph: LGraphId,
        parent_node: LNodeId,
        orig_edge: LEdgeId,
        opposite_port: LPortId,
        port_type: PortType,
        current: &mut Option<usize>,
        created: &mut Vec<ExternalPort>,
    ) {
        let merge_external_ports = self.arena.graphs[graph.0].props.merge_hierarchy_edges;

        // Does the edge end at the parent node itself?
        let mut parent_end_port: Option<LPortId> = None;
        if port_type == PortType::Input {
            let source = self.arena.edges[orig_edge.0].source.unwrap();
            if self.arena.ports[source.0].owner == Some(parent_node) {
                parent_end_port = Some(source);
            }
        } else {
            let target = self.arena.edges[orig_edge.0].target.unwrap();
            if self.arena.ports[target.0].owner == Some(parent_node) {
                parent_end_port = Some(target);
            }
        }

        let reuse = current.is_some() && merge_external_ports && parent_end_port.is_none();
        if reuse {
            let idx = current.unwrap();
            created[idx].orig_edges.push(orig_edge);
            // Java also merges EDGE_THICKNESS here (max) — thickness
            // is not modelled in scope (draw-uml never sets it).
            let new_edge = created[idx].new_edge;
            self.cross_hierarchy_map.entry(orig_edge).or_default().push(CrossHierarchyEdge {
                edge: new_edge,
                graph,
                port_type,
            });
            return;
        }

        let external_port_side = if let Some(pep) = parent_end_port {
            self.arena.ports[pep.0].side
        } else if self.arena.nodes[parent_node.0].props.port_constraints.is_side_fixed() {
            if port_type == PortType::Input { PortSide::West } else { PortSide::East }
        } else {
            PortSide::Undefined
        };

        let dummy_node = self.create_external_port_dummy(
            graph,
            parent_node,
            port_type,
            external_port_side,
            orig_edge,
        );
        let dummy_edge = self.create_dummy_edge(orig_edge);
        let dummy_node_port = self.arena.nodes[dummy_node.0].ports[0];
        if port_type == PortType::Input {
            self.arena.edge_set_source(dummy_edge, Some(dummy_node_port));
            self.arena.edge_set_target(dummy_edge, Some(opposite_port));
        } else {
            self.arena.edge_set_source(dummy_edge, Some(opposite_port));
            self.arena.edge_set_target(dummy_edge, Some(dummy_node_port));
        }

        let dummy_port = self.arena.nodes[dummy_node.0]
            .props
            .origin_port
            .expect("external port dummy must know its parent port");
        created.push(ExternalPort {
            orig_edges: vec![orig_edge],
            new_edge: dummy_edge,
            dummy_node,
            dummy_port,
            port_type,
            exported: parent_end_port.is_none(),
        });
        if parent_end_port.is_none() {
            *current = Some(created.len() - 1);
        }
        self.cross_hierarchy_map.entry(orig_edge).or_default().push(CrossHierarchyEdge {
            edge: dummy_edge,
            graph,
            port_type,
        });
    }

    /// Java `createDummyEdge`: property copy only (JUNCTION_POINTS
    /// reset is moot — never set in scope).
    fn create_dummy_edge(&mut self, orig_edge: LEdgeId) -> LEdgeId {
        let props = self.arena.edges[orig_edge.0].props.clone();
        let dummy = self.arena.new_edge();
        self.arena.edges[dummy.0].props = props;
        dummy
    }

    /// Java `CompoundGraphPreprocessor.createExternalPortDummy` — the
    /// wrapper that decides between reusing the parent's own end port
    /// and inventing a fresh parent port, then delegates to the
    /// `LGraphUtil` helper.
    fn create_external_port_dummy(
        &mut self,
        graph: LGraphId,
        parent_node: LNodeId,
        port_type: PortType,
        port_side: PortSide,
        edge: LEdgeId,
    ) -> LNodeId {
        let outside_port = if port_type == PortType::Input {
            self.arena.edges[edge.0].source.unwrap()
        } else {
            self.arena.edges[edge.0].target.unwrap()
        };
        let layout_direction = self.graph_direction(graph);

        let dummy_node;
        if self.arena.ports[outside_port.0].owner == Some(parent_node) {
            // Edge ends at the parent node itself: the dummy stands in
            // for the parent's own port.
            if let Some(&existing) = self.dummy_node_map.get(&outside_port) {
                dummy_node = existing;
            } else {
                let constraints = self.arena.nodes[parent_node.0].props.port_constraints;
                let net_flow = self.calculate_net_flow(outside_port);
                let position = self.arena.ports[outside_port.0].position;
                let size = self.arena.ports[outside_port.0].size;
                dummy_node = self.lgraphutil_create_external_port_dummy(
                    graph,
                    0.0, // parent's port has no PORT_BORDER_OFFSET in scope
                    constraints,
                    port_side,
                    net_flow,
                    position,
                    size,
                    layout_direction,
                );
                self.arena.nodes[dummy_node.0].props.origin_port = Some(outside_port);
                self.dummy_node_map.insert(outside_port, dummy_node);
            }
        } else {
            // Ordinary crossing: fresh dummy + fresh port on the
            // parent node.
            let border_offset = self.arena.graphs[graph.0].props.spacing.edge_edge / 2.0;
            let constraints = self.arena.nodes[parent_node.0].props.port_constraints;
            dummy_node = self.lgraphutil_create_external_port_dummy(
                graph,
                border_offset,
                constraints,
                port_side,
                if port_type == PortType::Input { -1 } else { 1 },
                KVector::default(),
                KVector::default(), // dummy ports have no size (#766)
                layout_direction,
            );
            let dummy_port = self.create_port_for_dummy(dummy_node, parent_node, port_type);
            self.arena.nodes[dummy_node.0].props.origin_port = Some(dummy_port);
            self.dummy_node_map.insert(dummy_port, dummy_node);
        }

        self.arena.graphs[graph.0].props.graph_properties.external_ports = true;
        let gp = self.arena.graphs[graph.0].props.port_constraints;
        self.arena.graphs[graph.0].props.port_constraints = if gp.is_side_fixed() {
            PortConstraints::FixedSide
        } else {
            PortConstraints::Free
        };
        dummy_node
    }

    /// Java `LGraphUtil.createExternalPortDummy` (the parts reachable
    /// in scope: no explicit PORT_ANCHOR, no FIXED_ORDER/RATIO parent
    /// constraints — asserted).
    #[allow(clippy::too_many_arguments)]
    fn lgraphutil_create_external_port_dummy(
        &mut self,
        graph: LGraphId,
        port_border_offset: f64,
        port_constraints: PortConstraints,
        port_side: PortSide,
        net_flow: i32,
        _port_position: KVector,
        port_size: KVector,
        layout_direction: Direction,
    ) -> LNodeId {
        assert!(
            !port_constraints.is_order_fixed(),
            "FIXED_ORDER/RATIO/POS parent constraints are outside the ported scope"
        );
        let mut final_side = port_side;
        let dummy = self.arena.new_node(graph);
        // new_node pushes into layerless_nodes; Java defers that to the
        // caller — pop it back out so commit_created_ports controls
        // membership exactly like upstream.
        self.arena.graphs[graph.0].layerless_nodes.pop();
        self.arena.nodes[dummy.0].node_type = NodeType::ExternalPort;
        self.arena.nodes[dummy.0].props.ext_port_size = port_size;
        self.arena.nodes[dummy.0].props.port_constraints = PortConstraints::FixedPos;
        self.arena.nodes[dummy.0].props.port_border_offset = port_border_offset;

        let dummy_port = self.arena.new_port(dummy);
        if !port_constraints.is_side_fixed() {
            assert!(layout_direction != Direction::Undefined);
            final_side = if net_flow >= 0 {
                PortSide::from_direction(layout_direction)
            } else {
                PortSide::from_direction(layout_direction).opposed()
            };
        }

        let mut anchor = KVector::new(port_size.x / 2.0, port_size.y / 2.0);
        match final_side {
            PortSide::West => {
                self.arena.nodes[dummy.0].props.layer_constraint = LayerConstraint::FirstSeparate;
                self.arena.nodes[dummy.0].props.edge_constraint = EdgeConstraint::OutgoingOnly;
                self.arena.nodes[dummy.0].size.y = port_size.y;
                if port_border_offset < 0.0 {
                    self.arena.nodes[dummy.0].size.x = -port_border_offset;
                }
                self.arena.port_set_side(dummy_port, PortSide::East);
                anchor.x = port_size.x;
                anchor.x -= port_size.x;
            }
            PortSide::East => {
                self.arena.nodes[dummy.0].props.layer_constraint = LayerConstraint::LastSeparate;
                self.arena.nodes[dummy.0].props.edge_constraint = EdgeConstraint::IncomingOnly;
                self.arena.nodes[dummy.0].size.y = port_size.y;
                if port_border_offset < 0.0 {
                    self.arena.nodes[dummy.0].size.x = -port_border_offset;
                }
                self.arena.port_set_side(dummy_port, PortSide::West);
                anchor.x = 0.0;
            }
            PortSide::North => {
                self.arena.nodes[dummy.0].props.in_layer_constraint = InLayerConstraint::Top;
                self.arena.nodes[dummy.0].size.x = port_size.x;
                if port_border_offset < 0.0 {
                    self.arena.nodes[dummy.0].size.y = -port_border_offset;
                }
                self.arena.port_set_side(dummy_port, PortSide::South);
                anchor.y = port_size.y;
                anchor.y -= port_size.y;
            }
            PortSide::South => {
                self.arena.nodes[dummy.0].props.in_layer_constraint = InLayerConstraint::Bottom;
                self.arena.nodes[dummy.0].size.x = port_size.x;
                if port_border_offset < 0.0 {
                    self.arena.nodes[dummy.0].size.y = -port_border_offset;
                }
                self.arena.port_set_side(dummy_port, PortSide::North);
                anchor.y = 0.0;
            }
            PortSide::Undefined => panic!("external port dummy needs a defined side"),
        }
        self.arena.ports[dummy_port.0].position = anchor;
        self.arena.nodes[dummy.0].props.port_anchor = Some(anchor);
        self.arena.nodes[dummy.0].props.ext_port_side = final_side;
        // Java `LGraphUtil.createExternalPortDummy`: a western (input)
        // dummy is layered FIRST_SEPARATE, an eastern (output) dummy
        // LAST_SEPARATE. Sides are still in the DOWN frame here (this port
        // skips the import rotation), so normalize NORTH→WEST/SOUTH→EAST.
        let normalized = match final_side {
            PortSide::North => PortSide::West,
            PortSide::South => PortSide::East,
            s => s,
        };
        self.arena.nodes[dummy.0].props.layer_constraint = match normalized {
            PortSide::West => LayerConstraint::FirstSeparate,
            PortSide::East => LayerConstraint::LastSeparate,
            _ => LayerConstraint::None,
        };
        dummy
    }

    /// Java `createPortForDummy`.
    fn create_port_for_dummy(
        &mut self,
        dummy_node: LNodeId,
        parent_node: LNodeId,
        port_type: PortType,
    ) -> LPortId {
        let graph = self.arena.nodes[parent_node.0].graph;
        let layout_direction = self.graph_direction(graph);
        let port = self.arena.new_port(parent_node);
        let side = match port_type {
            PortType::Input => PortSide::from_direction(layout_direction).opposed(),
            PortType::Output => PortSide::from_direction(layout_direction),
            PortType::Undefined => PortSide::Undefined,
        };
        self.arena.port_set_side(port, side);
        self.arena.ports[port.0].props.port_border_offset =
            self.arena.nodes[dummy_node.0].props.port_border_offset;
        port
    }

    /// Java `calculateNetFlow(port)` with inside-self-loops always
    /// disabled (out of scope).
    fn calculate_net_flow(&self, port: LPortId) -> i32 {
        let node = self.arena.ports[port.0].owner;
        let mut output_port_vote = 0;
        let mut input_port_vote = 0;
        for &out_edge in &self.arena.ports[port.0].outgoing_edges {
            if self.arena.edge_is_self_loop(out_edge) {
                output_port_vote += 1;
            } else {
                let target_node = self.arena.edge_target_node(out_edge).unwrap();
                let target_graph = self.arena.nodes[target_node.0].graph;
                if self.arena.graphs[target_graph.0].parent_node == node {
                    input_port_vote += 1;
                } else {
                    output_port_vote += 1;
                }
            }
        }
        for &in_edge in &self.arena.ports[port.0].incoming_edges {
            if self.arena.edge_is_self_loop(in_edge) {
                input_port_vote += 1;
            } else {
                let source_node = self.arena.edge_source_node(in_edge).unwrap();
                let source_graph = self.arena.nodes[source_node.0].graph;
                if self.arena.graphs[source_graph.0].parent_node == node {
                    output_port_vote += 1;
                } else {
                    input_port_vote += 1;
                }
            }
        }
        output_port_vote - input_port_vote
    }

    /// Java `LGraphUtil.isDescendant(node, parent)`: walks the nesting
    /// chain node → owning graph → parent node → …
    fn is_descendant(&self, node: LNodeId, parent: LNodeId) -> bool {
        let mut current = self.arena.nodes[node.0].graph;
        loop {
            match self.arena.graphs[current.0].parent_node {
                Some(p) if p == parent => return true,
                Some(p) => current = self.arena.nodes[p.0].graph,
                None => return false,
            }
        }
    }

    /// Java `LGraphUtil.getDirection(graph)`: RIGHT when undefined.
    fn graph_direction(&self, graph: LGraphId) -> Direction {
        match self.arena.graphs[graph.0].props.direction {
            Direction::Undefined => Direction::Right,
            d => d,
        }
    }
}

// ----------------------------------------------------------------------
// CompoundGraphPostprocessor
// ----------------------------------------------------------------------

/// Port of `CompoundGraphPostprocessor.process`: reassemble every
/// original cross-hierarchy edge from its dummy segments — bend points
/// are concatenated in the source port's graph frame (the *reference
/// graph*), hierarchy-junction source points are inserted where
/// consecutive segments actually bend, the original endpoints are
/// restored, and the dummy edges disconnected. Junction points and edge
/// labels never occur on cross-hierarchy edges in scope.
///
/// Returns each original edge's reference graph — the frame its restored
/// bend points live in, which the exporter converts per edge container.
/// (Java stores the equivalent `TARGET_OFFSET`; the absolute-frame
/// exporter recomputes it from the graph tree instead.)
pub fn postprocess(
    arena: &mut LGraphArena,
    top: LGraphId,
    cross_map: &HashMap<LEdgeId, Vec<CrossHierarchyEdge>>,
) -> HashMap<LEdgeId, LGraphId> {
    let mut reference_graphs: HashMap<LEdgeId, LGraphId> = HashMap::new();
    let mut dummy_edges: Vec<LEdgeId> = Vec::new();

    for (&orig_edge, segments) in cross_map {
        let mut segments: Vec<CrossHierarchyEdge> = segments.clone();
        segments.sort_by(|a, b| cross_hierarchy_edge_cmp(arena, a, b, top));

        let source_port = actual_source(arena, &segments[0]);
        let target_port = actual_target(arena, &segments[segments.len() - 1]);

        // Reference graph: the source node's graph, or its nested graph
        // when the target lies inside the source node.
        let reference_node = arena.ports[source_port.0].owner.unwrap();
        let target_node = arena.ports[target_port.0].owner.unwrap();
        let reference_graph = if hierarchical::is_descendant(arena, target_node, reference_node) {
            arena.nodes[reference_node.0].nested_graph.unwrap()
        } else {
            arena.nodes[reference_node.0].graph
        };
        reference_graphs.insert(orig_edge, reference_graph);

        arena.edges[orig_edge.0].bend_points.clear();
        let mut last_point: Option<crate::layout::elk::alg::math::KVector> = None;

        for ch in &segments {
            let offset = hierarchical::change_coord_system(arena, ch.graph, reference_graph);
            let ledge = ch.edge;
            let mut bend_points = arena.edges[ledge.0].bend_points.clone();
            for p in bend_points.iter_mut() {
                p.x += offset.x;
                p.y += offset.y;
            }

            let mut source_point =
                hierarchical::absolute_anchor(arena, arena.edges[ledge.0].source.unwrap());
            source_point.x += offset.x;
            source_point.y += offset.y;
            let mut target_point =
                hierarchical::absolute_anchor(arena, arena.edges[ledge.0].target.unwrap());
            target_point.x += offset.x;
            target_point.y += offset.y;

            if let Some(last) = last_point {
                let next_point = bend_points.first().copied().unwrap_or(target_point);
                // UNNECESSARY_BENDPOINTS is never set in scope (false):
                // insert the segment-junction point only when the chain
                // actually bends there in both axes.
                const TOLERANCE: f64 = 1e-3;
                let x_diff_enough = (last.x - next_point.x).abs() > TOLERANCE;
                let y_diff_enough = (last.y - next_point.y).abs() > TOLERANCE;
                if x_diff_enough && y_diff_enough {
                    arena.edges[orig_edge.0].bend_points.push(source_point);
                }
            }

            last_point = Some(bend_points.last().copied().unwrap_or(source_point));
            arena.edges[orig_edge.0].bend_points.extend(bend_points);

            copy_labels_back(arena, ledge, orig_edge, reference_graph);

            dummy_edges.push(ledge);
        }

        arena.edge_set_source(orig_edge, Some(source_port));
        arena.edge_set_target(orig_edge, Some(target_port));
    }

    // Disconnect the dummy edges (dummy ports and nodes are retained).
    dummy_edges.sort();
    dummy_edges.dedup();
    for dummy in dummy_edges {
        arena.edge_set_source(dummy, None);
        arena.edge_set_target(dummy, None);
    }

    reference_graphs
}

/// Java `CompoundGraphPostprocessor.copyLabelsBack`: move the labels the
/// preprocessor parked on this hierarchy segment back to their original
/// edge, converting their positions from the segment's source graph into
/// the reference graph's frame.
fn copy_labels_back(
    arena: &mut LGraphArena,
    hierarchy_segment: LEdgeId,
    orig_edge: LEdgeId,
    reference_graph: LGraphId,
) {
    let labels = arena.edges[hierarchy_segment.0].labels.clone();
    if labels.is_empty() {
        return;
    }
    let src_node = arena.edge_source_node(hierarchy_segment).unwrap();
    let src_graph = arena.nodes[src_node.0].graph;
    let mut remaining = Vec::new();
    for label in labels {
        if arena.labels[label.0].props.original_label_edge != Some(orig_edge) {
            remaining.push(label);
            continue;
        }
        let offset = hierarchical::change_coord_system(arena, src_graph, reference_graph);
        arena.labels[label.0].position.x += offset.x;
        arena.labels[label.0].position.y += offset.y;
        arena.edges[orig_edge.0].labels.push(label);
    }
    arena.edges[hierarchy_segment.0].labels = remaining;
}

/// Java `CompoundGraphPreprocessor.getShallowestEdgeSegment`: the last
/// OUTPUT segment of the sorted chain (the segment right before the
/// first INPUT one), or the first segment when the chain starts with an
/// INPUT segment; the last segment when no INPUT segment exists.
fn shallowest_edge_segment(segments: &[CrossHierarchyEdge]) -> Option<usize> {
    let mut result: Option<usize> = None;
    for (index, ch) in segments.iter().enumerate() {
        if ch.port_type == PortType::Input {
            result = Some(if index == 0 { 0 } else { index - 1 });
            break;
        } else if index == segments.len() - 1 {
            result = Some(index);
        }
    }
    result
}

/// Java `CrossHierarchyEdge.getActualSource`: an external-port dummy
/// endpoint stands in for the containing node's port.
fn actual_source(arena: &LGraphArena, ch: &CrossHierarchyEdge) -> LPortId {
    let port = arena.edges[ch.edge.0].source.unwrap();
    let node = arena.ports[port.0].owner.unwrap();
    if arena.nodes[node.0].node_type == NodeType::ExternalPort {
        arena.nodes[node.0].props.origin_port.unwrap()
    } else {
        port
    }
}

/// Java `CrossHierarchyEdge.getActualTarget`.
fn actual_target(arena: &LGraphArena, ch: &CrossHierarchyEdge) -> LPortId {
    let port = arena.edges[ch.edge.0].target.unwrap();
    let node = arena.ports[port.0].owner.unwrap();
    if arena.nodes[node.0].node_type == NodeType::ExternalPort {
        arena.nodes[node.0].props.origin_port.unwrap()
    } else {
        port
    }
}

/// Java `CrossHierarchyEdgeComparator`: OUTPUT segments before INPUT
/// segments; OUTPUT sorted from the deepest graph up, INPUT from the
/// top down.
fn cross_hierarchy_edge_cmp(
    arena: &LGraphArena,
    a: &CrossHierarchyEdge,
    b: &CrossHierarchyEdge,
    top: LGraphId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.port_type == PortType::Output && b.port_type == PortType::Input {
        return Ordering::Less;
    }
    if a.port_type == PortType::Input && b.port_type == PortType::Output {
        return Ordering::Greater;
    }
    let level = |mut g: LGraphId| {
        let mut lvl = 0i32;
        while g != top {
            let parent = arena.graphs[g.0]
                .parent_node
                .expect("segment graph must descend from the top-level graph");
            g = arena.nodes[parent.0].graph;
            lvl += 1;
        }
        lvl
    };
    let (l1, l2) = (level(a.graph), level(b.graph));
    if a.port_type == PortType::Output {
        l2.cmp(&l1)
    } else {
        l1.cmp(&l2)
    }
}
