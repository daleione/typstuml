//! Port of `org.eclipse.elk.alg.layered.graph.transform.ElkGraphImporter`
//! (v0.11.0), restricted to the graph shapes the draw-uml adapter
//! produces (see docs/elk-port-plan.md §2): node-to-node edges (no
//! explicit ELK ports anywhere), optional `INCLUDE_CHILDREN`
//! hierarchy, no inside self-loops, no external ports, no hypernodes.
//! Every out-of-scope construct is rejected loudly (`panic!`) instead
//! of being half-handled, so an oracle mismatch can never come from a
//! silently skipped feature.
//!
//! The JSON-side input is [`crate::layout::elk::graph::ElkNode`] —
//! this crate's equivalent of elkjs's input object graph.

use std::collections::HashMap;
use std::collections::VecDeque;

use serde_json::Value;

use crate::layout::elk::graph as json;

use super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId, LPortId};
use super::math::{Insets, KVector};
use super::options::{
    CycleBreakingStrategy, Direction, EdgeLabelPlacement, EdgeRouting, FixedAlignment,
    HierarchyHandling, OrderingStrategy, PortSide, PortType,
};

/// Java `ElkGraphImporter.importGraph(elkgraph)`: builds the LGraph
/// tree inside `arena` and returns the top-level graph. Panics on
/// constructs outside the ported scope.
pub fn import_graph(arena: &mut LGraphArena, elkgraph: &json::ElkNode) -> LGraphId {
    let mut importer = Importer { arena, node_map: HashMap::new() };
    importer.import_graph(elkgraph)
}

struct Importer<'a> {
    arena: &'a mut LGraphArena,
    /// Java `nodeAndPortMap` (ports never appear in scope, so it only
    /// ever maps nodes).
    node_map: HashMap<String, LNodeId>,
}

impl Importer<'_> {
    fn import_graph(&mut self, elkgraph: &json::ElkNode) -> LGraphId {
        assert!(
            elkgraph.ports.as_ref().is_none_or(|p| p.is_empty()),
            "external ports are outside the ported scope"
        );

        let top_level_graph = self.create_lgraph(elkgraph);

        // Java: ensureDefinedPortSide / checkExternalPorts /
        // transformExternalPort / calculateMinimumGraphSize /
        // partitioning / spacing base value — all no-ops for inputs
        // without root ports, size constraints, partitioning or
        // spacing base values (asserted where cheap).
        assert!(
            get_opt(elkgraph, "elk.partitioning.activate").is_none(),
            "partitioning is outside the ported scope"
        );

        if graph_props(self.arena, top_level_graph).hierarchy_handling
            == HierarchyHandling::IncludeChildren
        {
            self.import_hierarchical_graph(elkgraph, top_level_graph);
        } else {
            self.import_flat_graph(elkgraph, top_level_graph);
        }
        top_level_graph
    }

    /// Java `importFlatGraph`: direct children and edges only; edges
    /// must connect siblings (checked).
    fn import_flat_graph(&mut self, elkgraph: &json::ElkNode, lgraph: LGraphId) {
        let consider = needs_model_order_based_on_parent(elkgraph);
        let mut index = 0;
        for child in elkgraph.children.as_deref().unwrap_or(&[]) {
            assert!(
                child.children.as_ref().is_none_or(|c| c.is_empty()),
                "nested children without INCLUDE_CHILDREN are outside the ported scope"
            );
            let node = self.transform_node(child, lgraph);
            if consider {
                self.arena.nodes[node.0].props.model_order = Some(index);
                index += 1;
            }
        }
        self.arena.graphs[lgraph.0].props.max_model_order_nodes = index;

        let mut edge_index = 0;
        for elkedge in elkgraph.edges.as_deref().unwrap_or(&[]) {
            let ledge = self.transform_edge(elkedge, elkgraph, lgraph);
            if consider {
                self.arena.edges[ledge.0].props.model_order = Some(edge_index);
                edge_index += 1;
            }
        }
    }

    /// Java `importHierarchicalGraph`. Faithfully keeps its quirks:
    /// - a node's MODEL_ORDER eligibility is decided by its *direct
    ///   parent's* options (nested children of an option-less group
    ///   get none), while the node model-order *counter* is global;
    /// - edge MODEL_ORDER eligibility is decided by the *top-level*
    ///   graph's options for every hierarchy level.
    fn import_hierarchical_graph(&mut self, elkgraph: &json::ElkNode, lgraph: LGraphId) {
        let parent_graph_direction = graph_props(self.arena, lgraph).direction;

        // ---- node pass -------------------------------------------------
        let mut index = 0;
        // Queue entries: (child, its parent) — the parent decides
        // needsModelOrder and supplies the parent LGraph.
        let mut queue: VecDeque<(&json::ElkNode, &json::ElkNode)> = VecDeque::new();
        for child in elkgraph.children.as_deref().unwrap_or(&[]) {
            queue.push_back((child, elkgraph));
        }
        while let Some((elknode, elkparent)) = queue.pop_front() {
            if needs_model_order_based_on_parent(elkparent) {
                // (CONSIDER_MODEL_ORDER_NO_MODEL_ORDER is never set in
                // scope, so needsModelOrder == parent check.)
                // Assigned to the LNode after transform below; Java
                // stashes it on the ElkNode first.
            }

            let has_children = elknode.children.as_ref().is_some_and(|c| !c.is_empty());
            let has_hierarchy_handling_enabled = matches!(
                get_opt(elknode, "elk.hierarchyHandling").map(String::as_str),
                Some("INCLUDE_CHILDREN")
            ) || (
                // HierarchyHandling.INHERIT: children inherit the
                // parent's effective value; draw-uml sets the option
                // only at the root, so INHERIT + hierarchical import
                // running at all means it's enabled.
                get_opt(elknode, "elk.hierarchyHandling").is_none()
            );

            let mut nested_graph = None;
            if has_hierarchy_handling_enabled && has_children {
                let ng = self.create_lgraph(elknode);
                self.arena.graphs[ng.0].props.direction = parent_graph_direction;
                nested_graph = Some(ng);
            }

            let parent_lgraph = match self.node_map.get(&elkparent.id) {
                Some(&pl) => self.arena.nodes[pl.0]
                    .nested_graph
                    .expect("parent node must have a nested graph"),
                None => lgraph,
            };
            let lnode = self.transform_node(elknode, parent_lgraph);
            if needs_model_order_based_on_parent(elkparent) {
                self.arena.nodes[lnode.0].props.model_order = Some(index);
                index += 1;
            }

            if let Some(ng) = nested_graph {
                self.arena.nodes[lnode.0].nested_graph = Some(ng);
                self.arena.graphs[ng.0].parent_node = Some(lnode);
                for child in elknode.children.as_deref().unwrap_or(&[]) {
                    queue.push_back((child, elknode));
                }
            }
        }
        self.arena.graphs[lgraph.0].props.max_model_order_nodes = index;

        // ---- edge pass -------------------------------------------------
        let mut index = 0;
        let mut graph_queue: VecDeque<&json::ElkNode> = VecDeque::new();
        graph_queue.push_back(elkgraph);
        while let Some(elk_graph_node) = graph_queue.pop_front() {
            for elkedge in elk_graph_node.edges.as_deref().unwrap_or(&[]) {
                check_edge_validity(elkedge);

                let model_order = if needs_model_order_based_on_parent(elkgraph) {
                    let mo = index;
                    index += 1;
                    Some(mo)
                } else {
                    None
                };

                // Inside self-loops and descendant containment moves
                // the edge into an endpoint's nested graph. draw-uml
                // attaches every edge at the endpoints' LCA already
                // and never activates inside self-loops, so the edge
                // stays in the current graph — but descendant
                // containment is cheap to keep faithful:
                let source_id = &elkedge.sources[0];
                let target_id = &elkedge.targets[0];
                let parent_elk_graph = if is_descendant(elk_graph_node, target_id, source_id) {
                    source_id.as_str()
                } else if is_descendant(elk_graph_node, source_id, target_id) {
                    target_id.as_str()
                } else {
                    elk_graph_node.id.as_str()
                };

                let parent_lgraph = match self.node_map.get(parent_elk_graph) {
                    Some(&pl) => self.arena.nodes[pl.0]
                        .nested_graph
                        .expect("edge parent must have nested graph"),
                    None => lgraph,
                };

                let ledge = self.transform_edge(elkedge, elk_graph_node, parent_lgraph);
                if let Some(mo) = model_order {
                    self.arena.edges[ledge.0].props.model_order = Some(mo);
                }
                // Java also records COORDINATE_SYSTEM_ORIGIN here —
                // only consumed at export time; ported with the
                // exporter.
            }

            let has_hierarchy = elk_graph_node.id == elkgraph.id
                || !matches!(
                    get_opt(elk_graph_node, "elk.hierarchyHandling").map(String::as_str),
                    Some("SEPARATE_CHILDREN")
                );
            if has_hierarchy {
                for child in elk_graph_node.children.as_deref().unwrap_or(&[]) {
                    if child.children.as_ref().is_some_and(|c| !c.is_empty()) {
                        graph_queue.push_back(child);
                    }
                }
            }
        }
    }

    /// Java `createLGraph(elkgraph)`: fresh graph, properties copied,
    /// padding = `elk.padding` (label padding is zero in scope —
    /// draw-uml pre-computes group padding itself).
    fn create_lgraph(&mut self, elkgraph: &json::ElkNode) -> LGraphId {
        let g = self.arena.new_graph();
        let props = &mut self.arena.graphs[g.0].props;
        props.origin = Some(elkgraph.id.clone());
        parse_graph_options(props, elkgraph);
        if props.direction == Direction::Undefined {
            // Java: LGraphUtil.getDirection defaults to RIGHT.
            props.direction = Direction::Right;
        }
        self.arena.graphs[g.0].padding = parse_padding(elkgraph);
        g
    }

    /// Java `transformNode(elknode, lgraph)`.
    fn transform_node(&mut self, elknode: &json::ElkNode, lgraph: LGraphId) -> LNodeId {
        let lnode = self.arena.new_node(lgraph);
        let n = &mut self.arena.nodes[lnode.0];
        n.props.origin = Some(elknode.id.clone());
        n.size = KVector::new(elknode.width.unwrap_or(0.0), elknode.height.unwrap_or(0.0));
        n.position = KVector::new(elknode.x.unwrap_or(0.0), elknode.y.unwrap_or(0.0));
        n.props.compound_node = elknode.children.as_ref().is_some_and(|c| !c.is_empty());
        // PORT_CONSTRAINTS: UNDEFINED normalizes to FREE (scope: no
        // input ever sets it).
        assert!(
            get_opt(elknode, "elk.portConstraints").is_none(),
            "explicit port constraints are outside the ported scope"
        );
        assert!(
            elknode.ports.as_ref().is_none_or(|p| p.is_empty()),
            "explicit ports are outside the ported scope"
        );
        for elklabel in elknode.labels.as_deref().unwrap_or(&[]) {
            if !elklabel.text.is_empty() {
                let l = self.arena.new_label(elklabel.text.clone());
                self.arena.labels[l.0].size = KVector::new(
                    elklabel.width.unwrap_or(0.0),
                    elklabel.height.unwrap_or(0.0),
                );
                self.arena.labels[l.0].props.node_placement = elklabel
                    .layout_options
                    .as_ref()
                    .and_then(|o| o.get("elk.nodeLabels.placement"))
                    .and_then(|v| v.as_str().map(str::to_string));
                self.arena.nodes[lnode.0].labels.push(l);
            }
        }
        self.node_map.insert(elknode.id.clone(), lnode);
        lnode
    }

    /// Java `transformEdge(elkedge, elkparent, lgraph)` for port-less
    /// endpoints: both ends get a fresh anonymous port
    /// (`LGraphUtil.createPort` with `mergeEdges == false`), sides
    /// derived from the layout direction.
    fn transform_edge(
        &mut self,
        elkedge: &json::ElkEdge,
        _elkparent: &json::ElkNode,
        lgraph: LGraphId,
    ) -> super::graph::LEdgeId {
        check_edge_validity(elkedge);
        let source_lnode = *self
            .node_map
            .get(&elkedge.sources[0])
            .unwrap_or_else(|| panic!("unknown edge source {}", elkedge.sources[0]));
        let target_lnode = *self
            .node_map
            .get(&elkedge.targets[0])
            .unwrap_or_else(|| panic!("unknown edge target {}", elkedge.targets[0]));

        let ledge = self.arena.new_edge();
        self.arena.edges[ledge.0].props.origin = Some(elkedge.id.clone());

        if source_lnode == target_lnode {
            let g = &mut self.arena.graphs[lgraph.0].props.graph_properties;
            g.self_loops = true;
        }

        // Source port: created in the *edge's* graph; target port in
        // the *target node's* graph (Java quirk kept verbatim — the
        // graph argument only matters for mergeEdges/collector ports,
        // both false in scope, and for the NORTH_SOUTH_PORTS flag).
        let source_port = self.create_port(source_lnode, PortType::Output, lgraph);
        let target_graph = self.arena.nodes[target_lnode.0].graph;
        let target_port = self.create_port(target_lnode, PortType::Input, target_graph);
        self.arena.edge_set_source(ledge, Some(source_port));
        self.arena.edge_set_target(ledge, Some(target_port));

        for elklabel in elkedge.labels.as_deref().unwrap_or(&[]) {
            if !elklabel.text.is_empty() {
                let l = self.arena.new_label(elklabel.text.clone());
                self.arena.labels[l.0].size = KVector::new(
                    elklabel.width.unwrap_or(0.0),
                    elklabel.height.unwrap_or(0.0),
                );
                let get = |key: &str| -> Option<String> {
                    elklabel
                        .layout_options
                        .as_ref()?
                        .get(key)?
                        .as_str()
                        .map(str::to_string)
                };
                // `edgeLabels.inline` matters only when set on the *label*
                // element — draw-uml sets it on the edge, which ELK's
                // importer never propagates (elkjs-verified no-op).
                self.arena.labels[l.0].props.inline = matches!(
                    get("org.eclipse.elk.edgeLabels.inline").as_deref(),
                    Some("true")
                );
                let placement = get("org.eclipse.elk.edgeLabels.placement");
                match placement.as_deref() {
                    // EdgeLabelPlacement defaults to CENTER.
                    None | Some("CENTER") => {
                        self.arena.labels[l.0].props.placement = EdgeLabelPlacement::Center;
                        let g = &mut self.arena.graphs[lgraph.0].props.graph_properties;
                        g.center_labels = true;
                    }
                    Some("TAIL" | "HEAD") => panic!(
                        "head/tail edge labels (END_LABELS chain) are outside the ported scope"
                    ),
                    Some(other) => panic!("unknown edgeLabels.placement {other}"),
                }
                self.arena.edges[ledge.0].labels.push(l);
            }
        }
        ledge
    }

    /// Java `LGraphUtil.createPort(node, endPoint=null, type, graph)`
    /// for the non-merge, non-hypernode path.
    fn create_port(&mut self, node: LNodeId, port_type: PortType, graph: LGraphId) -> LPortId {
        let direction = graph_props(self.arena, graph).direction;
        let port = self.arena.new_port(node);
        let default_side = PortSide::from_direction(direction);
        let side =
            if port_type == PortType::Output { default_side } else { default_side.opposed() };
        self.arena.port_set_side(port, side);
        let north_south = match direction {
            Direction::Left | Direction::Right => {
                matches!(side, PortSide::North | PortSide::South)
            }
            Direction::Up | Direction::Down => matches!(side, PortSide::East | PortSide::West),
            Direction::Undefined => false,
        };
        if north_south {
            self.arena.graphs[graph.0].props.graph_properties.north_south_ports = true;
        }
        port
    }
}

fn graph_props<'a>(
    arena: &'a LGraphArena,
    g: LGraphId,
) -> &'a super::options::GraphProps {
    &arena.graphs[g.0].props
}

/// Java `checkEdgeValidity`: exactly one source and one target
/// (hyperedges unsupported by layered).
fn check_edge_validity(elkedge: &json::ElkEdge) {
    assert!(
        elkedge.sources.len() == 1 && elkedge.targets.len() == 1,
        "edge {} must have exactly one source and one target",
        elkedge.id
    );
}

/// Java `needsModelOrderBasedOnParent(elkgraph)` reduced to the
/// strategies representable in scope: any considerModelOrder strategy
/// other than NONE, or a model-order cycle breaker.
fn needs_model_order_based_on_parent(elkgraph: &json::ElkNode) -> bool {
    let consider = matches!(
        get_opt(elkgraph, "elk.layered.considerModelOrder.strategy").map(String::as_str),
        Some("NODES_AND_EDGES" | "PREFER_EDGES" | "PREFER_NODES")
    );
    let cycle = matches!(
        get_opt(elkgraph, "elk.layered.cycleBreaking.strategy").map(String::as_str),
        Some("MODEL_ORDER" | "GREEDY_MODEL_ORDER")
    );
    consider || cycle
}

/// Is `descendant_id` a strict descendant of `ancestor_id` within
/// `scope`'s subtree? (Java `ElkGraphUtil.isDescendant`, resolved over
/// the JSON tree.)
fn is_descendant(scope: &json::ElkNode, descendant_id: &str, ancestor_id: &str) -> bool {
    fn find<'a>(n: &'a json::ElkNode, id: &str) -> Option<&'a json::ElkNode> {
        n.find(id)
    }
    let Some(ancestor) = find(scope, ancestor_id) else {
        return false;
    };
    ancestor.id != descendant_id && ancestor.find(descendant_id).is_some()
}

fn get_opt<'a>(node: &'a json::ElkNode, key: &str) -> Option<&'a String> {
    match node.layout_options.as_ref()?.get(key)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn get_opt_value<'a>(node: &'a json::ElkNode, key: &str) -> Option<&'a Value> {
    node.layout_options.as_ref()?.get(key)
}

fn opt_f64(node: &json::ElkNode, key: &str) -> Option<f64> {
    match get_opt_value(node, key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn opt_bool(node: &json::ElkNode, key: &str) -> Option<bool> {
    match get_opt_value(node, key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Copy the option subset of the ported scope from JSON layoutOptions
/// into [`super::options::GraphProps`]. Unknown `elk.*` keys panic —
/// an option the port ignores but elkjs honors is a silent divergence
/// factory.
fn parse_graph_options(props: &mut super::options::GraphProps, node: &json::ElkNode) {
    const KNOWN: &[&str] = &[
        "elk.algorithm",
        "elk.direction",
        "elk.edgeRouting",
        "elk.hierarchyHandling",
        "elk.padding",
        "elk.contentAlignment",
        "elk.spacing.nodeNode",
        "elk.spacing.edgeNode",
        "elk.spacing.edgeEdge",
        "elk.spacing.edgeLabel",
        "elk.spacing.nodeSelfLoop",
        "elk.spacing.componentComponent",
        "elk.layered.spacing.nodeNodeBetweenLayers",
        "elk.layered.spacing.edgeNodeBetweenLayers",
        "elk.layered.spacing.edgeEdgeBetweenLayers",
        "elk.layered.considerModelOrder.strategy",
        "elk.layered.cycleBreaking.strategy",
        "elk.layered.mergeEdges",
        "elk.layered.nodePlacement.bk.fixedAlignment",
        "elk.layered.highDegreeNodes.treatment",
        "elk.layered.highDegreeNodes.threshold",
        "elk.layered.compaction.postCompaction.strategy",
    ];
    if let Some(opts) = node.layout_options.as_ref() {
        for key in opts.keys() {
            assert!(
                KNOWN.contains(&key.as_str()),
                "layout option {key} is outside the ported scope"
            );
        }
    }

    if let Some(algorithm) = get_opt(node, "elk.algorithm") {
        assert_eq!(algorithm, "layered", "only elk.layered is ported");
    }
    props.direction = match get_opt(node, "elk.direction").map(String::as_str) {
        Some("RIGHT") => Direction::Right,
        Some("LEFT") => Direction::Left,
        Some("DOWN") => Direction::Down,
        Some("UP") => Direction::Up,
        Some(other) => panic!("unknown elk.direction {other}"),
        None => Direction::Undefined,
    };
    props.edge_routing = match get_opt(node, "elk.edgeRouting").map(String::as_str) {
        Some("ORTHOGONAL") => EdgeRouting::Orthogonal,
        Some("POLYLINE") => EdgeRouting::Polyline,
        Some("SPLINES") => EdgeRouting::Splines,
        Some(other) => panic!("unknown elk.edgeRouting {other}"),
        None => EdgeRouting::Undefined,
    };
    props.hierarchy_handling =
        match get_opt(node, "elk.hierarchyHandling").map(String::as_str) {
            Some("INCLUDE_CHILDREN") => HierarchyHandling::IncludeChildren,
            Some("SEPARATE_CHILDREN") => HierarchyHandling::SeparateChildren,
            Some(other) => panic!("unknown elk.hierarchyHandling {other}"),
            None => HierarchyHandling::Inherit,
        };
    props.consider_model_order =
        match get_opt(node, "elk.layered.considerModelOrder.strategy").map(String::as_str) {
            Some("NODES_AND_EDGES") => OrderingStrategy::NodesAndEdges,
            Some("PREFER_EDGES") => OrderingStrategy::PreferEdges,
            Some("PREFER_NODES") => OrderingStrategy::PreferNodes,
            Some(other) => panic!("unknown considerModelOrder strategy {other}"),
            None => OrderingStrategy::None,
        };
    props.cycle_breaking =
        match get_opt(node, "elk.layered.cycleBreaking.strategy").map(String::as_str) {
            Some("GREEDY") | None => CycleBreakingStrategy::Greedy,
            Some("MODEL_ORDER") => CycleBreakingStrategy::ModelOrder,
            Some("GREEDY_MODEL_ORDER") => CycleBreakingStrategy::GreedyModelOrder,
            Some("DEPTH_FIRST") => CycleBreakingStrategy::DepthFirst,
            Some(other) => panic!("unknown cycleBreaking strategy {other}"),
        };
    props.merge_edges = opt_bool(node, "elk.layered.mergeEdges").unwrap_or(false);
    assert!(!props.merge_edges, "mergeEdges=true is outside the ported scope");
    props.post_compaction_left =
        match get_opt(node, "elk.layered.compaction.postCompaction.strategy").map(String::as_str) {
            Some("LEFT") => true,
            Some("NONE") | None => false,
            Some(other) => panic!("postCompaction strategy {other} is outside the ported scope"),
        };
    assert!(
        get_opt(node, "elk.layered.compaction.postCompaction.constraints").is_none_or(|v| v == "SCANLINE"),
        "non-SCANLINE post-compaction constraints are outside the ported scope"
    );
    props.favor_straight_edges =
        opt_bool(node, "elk.layered.nodePlacement.favorStraightEdges").unwrap_or(true);
    props.bk_fixed_alignment =
        match get_opt(node, "elk.layered.nodePlacement.bk.fixedAlignment").map(String::as_str) {
            Some("BALANCED") => FixedAlignment::Balanced,
            Some("NONE") | None => FixedAlignment::None,
            Some("LEFTUP") => FixedAlignment::LeftUp,
            Some("RIGHTUP") => FixedAlignment::RightUp,
            Some("LEFTDOWN") => FixedAlignment::LeftDown,
            Some("RIGHTDOWN") => FixedAlignment::RightDown,
            Some(other) => panic!("unknown bk.fixedAlignment {other}"),
        };
    props.high_degree_nodes_treatment =
        opt_bool(node, "elk.layered.highDegreeNodes.treatment").unwrap_or(false);
    props.high_degree_nodes_threshold =
        opt_f64(node, "elk.layered.highDegreeNodes.threshold").unwrap_or(16.0) as i32;

    let s = &mut props.spacing;
    macro_rules! spc {
        ($field:ident, $key:expr) => {
            if let Some(v) = opt_f64(node, $key) {
                s.$field = v;
            }
        };
    }
    spc!(node_node, "elk.spacing.nodeNode");
    spc!(edge_node, "elk.spacing.edgeNode");
    spc!(edge_edge, "elk.spacing.edgeEdge");
    spc!(edge_label, "elk.spacing.edgeLabel");
    spc!(node_self_loop, "elk.spacing.nodeSelfLoop");
    spc!(component_component, "elk.spacing.componentComponent");
    spc!(port_port, "elk.spacing.portPort");
    spc!(label_node, "elk.spacing.labelNode");
    spc!(node_node_between_layers, "elk.layered.spacing.nodeNodeBetweenLayers");
    spc!(edge_node_between_layers, "elk.layered.spacing.edgeNodeBetweenLayers");
    spc!(edge_edge_between_layers, "elk.layered.spacing.edgeEdgeBetweenLayers");
}

/// Export a fully laid-out **flat** LGraph back to the ELK JSON output
/// shape, applying ELK's after-phase-5 finish: `LongEdgeJoiner` +
/// `ReversedEdgeRestorer`, then the internal-rightward → user-`DOWN`
/// coordinate transform (`GraphTransformer`, `TO_INPUT_DIRECTION`).
///
/// The transform is a pure axis swap plus a padding-aligned translation
/// (verified byte-exact against elkjs for nodes and every edge section):
/// `out.x = internal.y − minY + pad.left`, `out.y = internal.x − minX +
/// pad.top`, sizes swapped. `minY`/`minX` are the content extent minima
/// **including** long-edge dummies (they bound routed vertical segments),
/// so the extent is snapshotted before the joiner removes them.
///
/// Scope: flat, single graph, `direction = DOWN`. Hierarchical export
/// (nested-graph coordinate transfer + `CompoundGraphPostprocessor`) is
/// E8 follow-up.
pub fn apply_layout(arena: &mut LGraphArena, graph: LGraphId) -> json::ElkNode {
    assert!(
        matches!(arena.graphs[graph.0].props.direction, Direction::Down),
        "apply_layout currently exports the DOWN orientation only"
    );

    // Content extent over every laid-out node (dummies included), in the
    // internal rightward frame: y is the in-layer axis, x the layer axis.
    let (mut min_y, mut min_x) = (f64::INFINITY, f64::INFINITY);
    let (mut max_y, mut max_x) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for layer in &arena.graphs[graph.0].layers {
        for &n in &layer.nodes {
            let node = &arena.nodes[n.0];
            min_y = min_y.min(node.position.y - node.margin.top);
            min_x = min_x.min(node.position.x - node.margin.left);
            max_y = max_y.max(node.position.y + node.size.y + node.margin.bottom);
            max_x = max_x.max(node.position.x + node.size.x + node.margin.right);
        }
    }
    let pad = arena.graphs[graph.0].padding;
    let off_x = pad.left - min_y;
    let off_y = pad.top - min_x;
    // internal (ix, iy) → output DOWN (iy + off_x, ix + off_y).
    let tx = |v: KVector| json::ElkPoint { x: v.y + off_x, y: v.x + off_y };

    // Finish: reassemble split edges, place + strip label dummies (the
    // LABEL_DUMMY_REMOVER slot follows LONG_EDGE_JOINER), restore
    // reversed edges.
    super::intermediate::long_edge_joiner(arena, graph);
    if arena.graphs[graph.0].props.graph_properties.center_labels {
        super::intermediate::label_dummy_remover(arena, graph);
    }
    super::intermediate::reversed_edge_restorer(arena, graph);

    // Real nodes → children (size axes swapped for the transpose).
    let mut children = Vec::new();
    for layer in &arena.graphs[graph.0].layers {
        for &n in &layer.nodes {
            let node = &arena.nodes[n.0];
            let Some(origin) = node.props.origin.clone() else { continue };
            children.push(json::ElkNode {
                id: origin,
                x: Some(node.position.y + off_x),
                y: Some(node.position.x + off_y),
                width: Some(node.size.y),
                height: Some(node.size.x),
                ..Default::default()
            });
        }
    }

    // Surviving original edges → one section each.
    let root_id = arena.graphs[graph.0].props.origin.clone().unwrap_or_else(|| "root".into());
    let mut edges = Vec::new();
    for e in 0..arena.edges.len() {
        let edge = &arena.edges[e];
        let (Some(src), Some(tgt)) = (edge.source, edge.target) else { continue };
        let Some(origin) = edge.props.origin.clone() else { continue };
        // Keep only surviving original edges of this graph (a dropped
        // dummy edge has no endpoints; a foreign edge's source lives in
        // another graph).
        let owner = arena.ports[src.0].owner.unwrap();
        if arena.nodes[owner.0].graph != graph {
            continue;
        }
        let bends: Vec<json::ElkPoint> = edge.bend_points.iter().map(|&b| tx(b)).collect();
        // Edge labels: placed positions are internal-frame graph
        // coordinates (like bend points); extents stay in the user frame.
        let labels: Vec<json::ElkLabel> = edge
            .labels
            .iter()
            .map(|&l| {
                let lab = &arena.labels[l.0];
                let p = tx(lab.position);
                json::ElkLabel {
                    text: lab.text.clone(),
                    x: Some(p.x),
                    y: Some(p.y),
                    width: Some(lab.size.x),
                    height: Some(lab.size.y),
                    ..Default::default()
                }
            })
            .collect();
        edges.push(json::ElkEdge {
            id: origin,
            sources: vec![arena.nodes[arena.ports[src.0].owner.unwrap().0]
                .props
                .origin
                .clone()
                .unwrap_or_default()],
            targets: vec![arena.nodes[arena.ports[tgt.0].owner.unwrap().0]
                .props
                .origin
                .clone()
                .unwrap_or_default()],
            sections: Some(vec![json::ElkEdgeSection {
                start_point: tx(arena.port_absolute_anchor(src)),
                end_point: tx(arena.port_absolute_anchor(tgt)),
                bend_points: if bends.is_empty() { None } else { Some(bends) },
                ..Default::default()
            }]),
            labels: if labels.is_empty() { None } else { Some(labels) },
            container: Some(root_id.clone()),
            ..Default::default()
        });
    }

    json::ElkNode {
        id: root_id,
        x: Some(0.0),
        y: Some(0.0),
        width: Some((max_y - min_y) + pad.left + pad.right),
        height: Some((max_x - min_x) + pad.top + pad.bottom),
        children: Some(children),
        edges: Some(edges),
        ..Default::default()
    }
}

/// Export a finished compound layout (`hierarchical::layout_compound`)
/// back onto the *input* graph's tree — the output mirrors the input's
/// node/edge containment, exactly like elkjs writes results onto the
/// caller's graph. All arena coordinates are internal-rightward; each
/// point is expressed absolutely (up-walking offsets, internal paddings
/// and parent-node positions), converted into its JSON container's
/// border frame, and transposed into the DOWN output frame.
pub fn apply_layout_compound(
    arena: &LGraphArena,
    top: LGraphId,
    input: &json::ElkNode,
    reference_graphs: &std::collections::HashMap<LEdgeId, LGraphId>,
) -> json::ElkNode {
    assert!(
        matches!(arena.graphs[top.0].props.direction, Direction::Down),
        "apply_layout_compound currently exports the DOWN orientation only"
    );

    // origin id → arena element. Disconnected dummy segments (postprocess)
    // drop out of the edge map; nodes without an origin are dummies.
    let mut node_map: HashMap<String, LNodeId> = HashMap::new();
    for (i, node) in arena.nodes.iter().enumerate() {
        if let Some(o) = node.props.origin.clone() {
            node_map.insert(o, LNodeId(i));
        }
    }
    let mut edge_map: HashMap<String, LEdgeId> = HashMap::new();
    for (i, edge) in arena.edges.iter().enumerate() {
        if edge.source.is_some() && edge.target.is_some() {
            if let Some(o) = edge.props.origin.clone() {
                edge_map.insert(o, LEdgeId(i));
            }
        }
    }

    // `A(g)`: the vector from a g-frame point to the absolute frame
    // (`changeCoordSystem`'s up-walk). `D(g)`: absolute origin of g's
    // border box, so json = abs − D(container).
    let a_of = |mut g: LGraphId| -> KVector {
        let mut v = KVector::default();
        loop {
            v = v.add(arena.graphs[g.0].offset);
            let Some(parent) = arena.graphs[g.0].parent_node else { break };
            let pad = super::hierarchical::internal_padding(arena, g);
            v.x += pad.left + arena.nodes[parent.0].position.x;
            v.y += pad.top + arena.nodes[parent.0].position.y;
            g = arena.nodes[parent.0].graph;
        }
        v
    };
    let d_of = |g: LGraphId| -> KVector {
        let a = a_of(g);
        let pad = super::hierarchical::internal_padding(arena, g);
        KVector::new(
            a.x - arena.graphs[g.0].offset.x - pad.left,
            a.y - arena.graphs[g.0].offset.y - pad.top,
        )
    };
    // internal (ix, iy) → output DOWN (iy, ix).
    let tx = |v: KVector| json::ElkPoint { x: v.y, y: v.x };

    fn emit(
        arena: &LGraphArena,
        input: &json::ElkNode,
        graph: LGraphId,
        is_top: bool,
        node_map: &HashMap<String, LNodeId>,
        edge_map: &HashMap<String, LEdgeId>,
        reference_graphs: &std::collections::HashMap<LEdgeId, LGraphId>,
        a_of: &dyn Fn(LGraphId) -> KVector,
        d_of: &dyn Fn(LGraphId) -> KVector,
        tx: &dyn Fn(KVector) -> json::ElkPoint,
    ) -> json::ElkNode {
        let mut out = json::ElkNode { id: input.id.clone(), ..Default::default() };

        if is_top {
            let size = super::hierarchical::actual_size(arena, graph);
            out.x = Some(0.0);
            out.y = Some(0.0);
            out.width = Some(size.y);
            out.height = Some(size.x);
        } else {
            let n = node_map[&input.id];
            let parent_graph = arena.nodes[n.0].graph;
            let pad = super::hierarchical::internal_padding(arena, parent_graph);
            let local = KVector::new(
                arena.nodes[n.0].position.x + arena.graphs[parent_graph.0].offset.x + pad.left,
                arena.nodes[n.0].position.y + arena.graphs[parent_graph.0].offset.y + pad.top,
            );
            let p = tx(local);
            out.x = Some(p.x);
            out.y = Some(p.y);
            out.width = Some(arena.nodes[n.0].size.y);
            out.height = Some(arena.nodes[n.0].size.x);
            // Node labels: position is node-relative; the label's placed
            // internal position transposes like everything else, extents
            // stay as imported (output frame).
            let labels: Vec<json::ElkLabel> = arena.nodes[n.0]
                .labels
                .iter()
                .map(|&l| {
                    let lab = &arena.labels[l.0];
                    json::ElkLabel {
                        text: lab.text.clone(),
                        x: Some(lab.position.y),
                        y: Some(lab.position.x),
                        width: Some(lab.size.x),
                        height: Some(lab.size.y),
                        ..Default::default()
                    }
                })
                .collect();
            if !labels.is_empty() {
                out.labels = Some(labels);
            }
        }

        // Children mirror the input containment.
        let mut children = Vec::new();
        for child in input.children.as_deref().unwrap_or(&[]) {
            let child_node = node_map[&child.id];
            let child_graph = arena.nodes[child_node.0].nested_graph.unwrap_or(graph);
            children.push(emit(
                arena,
                child,
                child_graph,
                false,
                node_map,
                edge_map,
                reference_graphs,
                a_of,
                d_of,
                tx,
            ));
        }
        if !children.is_empty() {
            out.children = Some(children);
        }

        // Edges contained here: points absolute → this container's frame.
        let container_d = d_of(graph);
        let mut edges = Vec::new();
        for elkedge in input.edges.as_deref().unwrap_or(&[]) {
            let ledge = edge_map[&elkedge.id];
            let src = arena.edges[ledge.0].source.unwrap();
            let tgt = arena.edges[ledge.0].target.unwrap();
            let src_graph = arena.nodes[arena.ports[src.0].owner.unwrap().0].graph;
            let tgt_graph = arena.nodes[arena.ports[tgt.0].owner.unwrap().0].graph;
            // Bend points live in the postprocessor's reference graph for
            // reassembled cross edges, in the edge's own graph otherwise.
            let bend_graph = reference_graphs.get(&ledge).copied().unwrap_or(src_graph);

            let to_container = |p: KVector, from: LGraphId| -> json::ElkPoint {
                let a = a_of(from);
                tx(KVector::new(p.x + a.x - container_d.x, p.y + a.y - container_d.y))
            };
            let start = to_container(arena.port_absolute_anchor(src), src_graph);
            let end = to_container(arena.port_absolute_anchor(tgt), tgt_graph);
            let bends: Vec<json::ElkPoint> = arena.edges[ledge.0]
                .bend_points
                .iter()
                .map(|&b| to_container(b, bend_graph))
                .collect();
            // Edge labels share the bend points' frame: the postprocessor
            // normalized cross-hierarchy labels into the reference graph;
            // plain edges' labels live in their own (source) graph.
            let labels: Vec<json::ElkLabel> = arena.edges[ledge.0]
                .labels
                .iter()
                .map(|&l| {
                    let lab = &arena.labels[l.0];
                    let p = to_container(lab.position, bend_graph);
                    json::ElkLabel {
                        text: lab.text.clone(),
                        x: Some(p.x),
                        y: Some(p.y),
                        width: Some(lab.size.x),
                        height: Some(lab.size.y),
                        ..Default::default()
                    }
                })
                .collect();

            edges.push(json::ElkEdge {
                id: elkedge.id.clone(),
                sources: elkedge.sources.clone(),
                targets: elkedge.targets.clone(),
                sections: Some(vec![json::ElkEdgeSection {
                    start_point: start,
                    end_point: end,
                    bend_points: if bends.is_empty() { None } else { Some(bends) },
                    ..Default::default()
                }]),
                labels: if labels.is_empty() { None } else { Some(labels) },
                container: Some(input.id.clone()),
                ..Default::default()
            });
        }
        if !edges.is_empty() {
            out.edges = Some(edges);
        }

        out
    }

    emit(
        arena,
        input,
        top,
        true,
        &node_map,
        &edge_map,
        reference_graphs,
        &a_of,
        &d_of,
        &tx,
    )
}

/// Parse `elk.padding` — `"[top=50,left=30,bottom=30,right=30]"`.
fn parse_padding(node: &json::ElkNode) -> Insets {
    let Some(raw) = get_opt(node, "elk.padding") else {
        // CoreOptions.PADDING default for layered is 12 on each side.
        return Insets { top: 12.0, right: 12.0, bottom: 12.0, left: 12.0 };
    };
    let mut padding = Insets::default();
    for part in raw.trim_matches(['[', ']']).split(',') {
        let Some((k, v)) = part.split_once('=') else { continue };
        let v: f64 = v.trim().parse().unwrap_or(0.0);
        match k.trim() {
            "top" => padding.top = v,
            "left" => padding.left = v,
            "bottom" => padding.bottom = v,
            "right" => padding.right = v,
            _ => {}
        }
    }
    padding
}
