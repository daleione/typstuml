//! Ports of the option enums the layered scope needs, from
//! `org.eclipse.elk.core.options` (`PortSide`, `PortConstraints`,
//! `EdgeRouting`, `HierarchyHandling`) and
//! `org.eclipse.elk.alg.layered.options` (`PortType`).

/// `org.eclipse.elk.core.options.PortSide`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PortSide {
    #[default]
    Undefined,
    North,
    East,
    South,
    West,
}

impl PortSide {
    /// Java `PortSide.opposed()`.
    pub fn opposed(self) -> PortSide {
        match self {
            PortSide::North => PortSide::South,
            PortSide::East => PortSide::West,
            PortSide::South => PortSide::North,
            PortSide::West => PortSide::East,
            PortSide::Undefined => PortSide::Undefined,
        }
    }
}

/// `org.eclipse.elk.alg.layered.options.PortType`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PortType {
    #[default]
    Undefined,
    Input,
    Output,
}

/// `org.eclipse.elk.core.options.PortConstraints`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PortConstraints {
    #[default]
    Undefined,
    Free,
    FixedSide,
    FixedOrder,
    FixedRatio,
    FixedPos,
}

impl PortConstraints {
    pub fn is_side_fixed(self) -> bool {
        !matches!(self, PortConstraints::Undefined | PortConstraints::Free)
    }
    pub fn is_order_fixed(self) -> bool {
        matches!(
            self,
            PortConstraints::FixedOrder | PortConstraints::FixedRatio | PortConstraints::FixedPos
        )
    }
    pub fn is_ratio_fixed(self) -> bool {
        matches!(self, PortConstraints::FixedRatio)
    }
    pub fn is_pos_fixed(self) -> bool {
        matches!(self, PortConstraints::FixedPos)
    }
}

/// `org.eclipse.elk.core.options.HierarchyHandling`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HierarchyHandling {
    #[default]
    Inherit,
    IncludeChildren,
    SeparateChildren,
}

/// `org.eclipse.elk.core.options.EdgeRouting`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EdgeRouting {
    #[default]
    Undefined,
    Polyline,
    Orthogonal,
    Splines,
}

/// `org.eclipse.elk.core.options.Direction`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Undefined,
    Right,
    Left,
    Down,
    Up,
}

impl PortSide {
    /// Java `PortSide.fromDirection(direction)` — the side an OUTPUT
    /// port defaults to under the given layout direction.
    pub fn from_direction(direction: Direction) -> PortSide {
        match direction {
            Direction::Right => PortSide::East,
            Direction::Left => PortSide::West,
            Direction::Down => PortSide::South,
            Direction::Up => PortSide::North,
            Direction::Undefined => PortSide::Undefined,
        }
    }
}

/// `org.eclipse.elk.alg.layered.options.OrderingStrategy`
/// (`considerModelOrder.strategy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderingStrategy {
    #[default]
    None,
    NodesAndEdges,
    PreferEdges,
    PreferNodes,
}

/// `org.eclipse.elk.alg.layered.options.FixedAlignment`
/// (`nodePlacement.bk.fixedAlignment`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FixedAlignment {
    #[default]
    None,
    LeftUp,
    RightUp,
    LeftDown,
    RightDown,
    Balanced,
}

/// `org.eclipse.elk.alg.layered.options.CycleBreakingStrategy` —
/// only the strategies reachable in the ported scope; the rest exist
/// so option parsing can fail loudly instead of misconfiguring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CycleBreakingStrategy {
    #[default]
    Greedy,
    DepthFirst,
    Interactive,
    ModelOrder,
    GreedyModelOrder,
}

/// `org.eclipse.elk.alg.layered.options.LayerConstraint`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LayerConstraint {
    #[default]
    None,
    First,
    FirstSeparate,
    Last,
    LastSeparate,
}

/// `org.eclipse.elk.alg.layered.options.InLayerConstraint`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InLayerConstraint {
    #[default]
    None,
    Top,
    Bottom,
}

/// `org.eclipse.elk.alg.layered.options.EdgeConstraint`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EdgeConstraint {
    #[default]
    None,
    IncomingOnly,
    OutgoingOnly,
}

/// The `InternalProperties.GRAPH_PROPERTIES` enum set — structural
/// facts the importer discovers, consumed by the processor
/// configurator later.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphPropertiesSet {
    pub comments: bool,
    pub external_ports: bool,
    pub hyperedges: bool,
    pub hypernodes: bool,
    pub non_free_ports: bool,
    pub north_south_ports: bool,
    pub self_loops: bool,
    pub center_labels: bool,
    pub end_labels: bool,
    pub partitions: bool,
}

/// `org.eclipse.elk.core.options.LabelSide` (the `InternalProperties.
/// LABEL_SIDE` values reachable through the center-label chain).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LabelSide {
    /// Java `LabelSide.UNKNOWN` (property default).
    #[default]
    Unknown,
    Above,
    Below,
    Inline,
}

/// The layered spacing options the draw-uml scope sets (Java keeps
/// them as individual `IProperty` entries; defaults from
/// `LayeredOptions`/`CoreOptions`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingProps {
    /// `elk.spacing.nodeNode` (default 20)
    pub node_node: f64,
    /// `elk.layered.spacing.nodeNodeBetweenLayers` (default 20)
    pub node_node_between_layers: f64,
    /// `elk.spacing.edgeNode` (default 10)
    pub edge_node: f64,
    /// `elk.layered.spacing.edgeNodeBetweenLayers` (default 10)
    pub edge_node_between_layers: f64,
    /// `elk.spacing.edgeEdge` (default 10)
    pub edge_edge: f64,
    /// `elk.layered.spacing.edgeEdgeBetweenLayers` (default 10)
    pub edge_edge_between_layers: f64,
    /// `elk.spacing.edgeLabel` (default 2)
    pub edge_label: f64,
    /// `elk.spacing.labelLabel` (default 0)
    pub label_label: f64,
    /// `elk.spacing.labelPortVertical` (default 1)
    pub label_port_vertical: f64,
    /// `elk.spacing.labelPortHorizontal` (default 1)
    pub label_port_horizontal: f64,
    /// `elk.spacing.nodeSelfLoop` (default 10)
    pub node_self_loop: f64,
    /// `elk.spacing.componentComponent` (default 20)
    pub component_component: f64,
    /// `elk.spacing.portPort` (default 10)
    pub port_port: f64,
    /// `elk.spacing.labelNode` (default 5)
    pub label_node: f64,
}

impl Default for SpacingProps {
    fn default() -> Self {
        Self {
            node_node: 20.0,
            node_node_between_layers: 20.0,
            edge_node: 10.0,
            edge_node_between_layers: 10.0,
            edge_edge: 10.0,
            edge_edge_between_layers: 10.0,
            edge_label: 2.0,
            label_label: 0.0,
            label_port_vertical: 1.0,
            label_port_horizontal: 1.0,
            node_self_loop: 10.0,
            component_component: 20.0,
            port_port: 10.0,
            label_node: 5.0,
        }
    }
}

/// Per-element property bags. Java keeps every option and every
/// internal (`InternalProperties`) marker in one `IProperty` hash map
/// per element; the port uses plain structs and adds fields as
/// milestones need them, keeping the Java option name in a comment.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphProps {
    /// `InternalProperties.ORIGIN` — external (JSON) id.
    pub origin: Option<String>,
    /// `elk.hierarchyHandling`
    pub hierarchy_handling: HierarchyHandling,
    /// `elk.edgeRouting`
    pub edge_routing: EdgeRouting,
    /// `elk.direction`
    pub direction: Direction,
    /// `elk.randomSeed` (layered uses it for tie-breaking)
    pub random_seed: u64,
    /// `elk.layered.considerModelOrder.strategy`
    pub consider_model_order: OrderingStrategy,
    /// `elk.layered.cycleBreaking.strategy`
    pub cycle_breaking: CycleBreakingStrategy,
    /// `elk.layered.mergeEdges`
    pub merge_edges: bool,
    /// `elk.separateConnectedComponents` (ELK default: true). Gates
    /// `ComponentsProcessor.split` on the flat layout path.
    pub separate_connected_components: bool,
    /// `elk.layered.nodePlacement.bk.fixedAlignment`
    pub bk_fixed_alignment: FixedAlignment,
    /// `elk.layered.compaction.postCompaction.strategy` — only NONE
    /// (default) and LEFT occur in scope; the constraint calculation
    /// stays at its SCANLINE default.
    pub post_compaction_left: bool,
    /// `elk.layered.nodePlacement.favorStraightEdges` (ELK default true).
    /// With alignment NONE it suppresses the balanced layout, making the
    /// node placer pick the smallest feasible of the four sweeps.
    pub favor_straight_edges: bool,
    /// `elk.layered.highDegreeNodes.treatment` / `.threshold` /
    /// `.treeHeight`
    pub high_degree_nodes_treatment: bool,
    pub high_degree_nodes_threshold: i32,
    pub high_degree_nodes_tree_height: i32,
    /// `elk.layered.thoroughness` (iteration-limit factor)
    pub thoroughness: i32,
    /// spacing group
    pub spacing: SpacingProps,
    /// `InternalProperties.GRAPH_PROPERTIES`
    pub graph_properties: GraphPropertiesSet,
    /// `InternalProperties.MAX_MODEL_ORDER_NODES`
    pub max_model_order_nodes: i32,
    /// `elk.layered.mergeHierarchyEdges` (Java default: true)
    pub merge_hierarchy_edges: bool,
    /// Graph-level `elk.portConstraints` — `createExternalPortDummy`
    /// stamps FIXED_SIDE/FREE onto the *graph* to signal external-port
    /// handling downstream.
    pub port_constraints: PortConstraints,
}

impl Default for GraphProps {
    fn default() -> Self {
        Self {
            origin: None,
            hierarchy_handling: HierarchyHandling::default(),
            edge_routing: EdgeRouting::default(),
            direction: Direction::default(),
            random_seed: 0,
            consider_model_order: OrderingStrategy::default(),
            cycle_breaking: CycleBreakingStrategy::default(),
            merge_edges: false,
            // ELK LayeredOptions.SEPARATE_CONNECTED_COMPONENTS default.
            separate_connected_components: true,
            bk_fixed_alignment: FixedAlignment::default(),
            post_compaction_left: false,
            favor_straight_edges: true,
            high_degree_nodes_treatment: false,
            high_degree_nodes_threshold: 16,
            // LayeredOptions.HIGH_DEGREE_NODES_TREE_HEIGHT default.
            high_degree_nodes_tree_height: 5,
            // LayeredOptions.THOROUGHNESS default.
            thoroughness: 7,
            spacing: SpacingProps::default(),
            graph_properties: GraphPropertiesSet::default(),
            max_model_order_nodes: 0,
            // LayeredOptions.MERGE_HIERARCHY_EDGES defaults to true.
            merge_hierarchy_edges: true,
            port_constraints: PortConstraints::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeProps {
    /// `InternalProperties.ORIGIN` — the external (JSON) id this node
    /// was imported from; the oracle comparison keys on it.
    pub origin: Option<String>,
    /// `InternalProperties.ORIGIN` when it points at a *port* — an
    /// external-port dummy's origin is the parent node's port it
    /// stands in for (Java's ORIGIN is polymorphic).
    pub origin_port: Option<crate::layout::elk::alg::graph::LPortId>,
    /// `InternalProperties.ORIGIN` when it points at an *edge* — a
    /// `LONG_EDGE` dummy's origin is the edge it splits (Java's ORIGIN
    /// is polymorphic; `LongEdgeJoiner` reads it back in E8).
    pub origin_edge: Option<crate::layout::elk::alg::graph::LEdgeId>,
    /// `elk.portConstraints`
    pub port_constraints: PortConstraints,
    /// `InternalProperties.MODEL_ORDER`
    pub model_order: Option<i32>,
    /// `InternalProperties.COMPOUND_NODE`
    pub compound_node: bool,
    /// `InternalProperties.EXT_PORT_SIDE`
    pub ext_port_side: PortSide,
    /// `InternalProperties.EXT_PORT_SIZE`
    pub ext_port_size: crate::layout::elk::alg::math::KVector,
    /// `elk.port.borderOffset` (stamped on external-port dummies)
    pub port_border_offset: f64,
    /// `elk.port.anchor` (stamped on external-port dummies)
    pub port_anchor: Option<crate::layout::elk::alg::math::KVector>,
    /// `elk.layered.layering.layerConstraint`
    pub layer_constraint: LayerConstraint,
    /// `InternalProperties.IN_LAYER_CONSTRAINT`
    pub in_layer_constraint: InLayerConstraint,
    /// `InternalProperties.EDGE_CONSTRAINT`
    pub edge_constraint: EdgeConstraint,
    /// `InternalProperties.LONG_EDGE_SOURCE` / `LONG_EDGE_TARGET` — on a
    /// `LONG_EDGE` dummy, the original edge's real source/target ports,
    /// carried through the dummy chain so `LongEdgeJoiner` (E8) can
    /// reconstruct the routed edge.
    pub long_edge_source: Option<crate::layout::elk::alg::graph::LPortId>,
    pub long_edge_target: Option<crate::layout::elk::alg::graph::LPortId>,
    /// `InternalProperties.LONG_EDGE_HAS_LABEL_DUMMIES` — the long edge
    /// this dummy is part of also carries a `LABEL` dummy.
    pub long_edge_has_label_dummies: bool,
    /// `InternalProperties.LONG_EDGE_BEFORE_LABEL_DUMMY` — this long-edge
    /// dummy precedes its edge's label dummy (only the hyperedge dummy
    /// merger reads it; kept for faithfulness).
    pub long_edge_before_label_dummy: bool,
    /// `InternalProperties.REPRESENTED_LABELS` — the center edge labels a
    /// `LABEL` dummy reserves space for.
    pub represented_labels: Vec<crate::layout::elk::alg::graph::LLabelId>,
    /// `InternalProperties.LABEL_SIDE` — which side of its edge a `LABEL`
    /// dummy's labels are placed on.
    pub label_side: LabelSide,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortProps {
    /// `InternalProperties.ORIGIN`
    pub origin: Option<String>,
    /// `InternalProperties.INPUT_COLLECT` (hypernode handling; unused
    /// in the draw-uml scope — `LEdge::reverse` panics if it ever sees
    /// it so a scope violation cannot pass silently)
    pub input_collect: bool,
    /// `InternalProperties.OUTPUT_COLLECT`
    pub output_collect: bool,
    /// `InternalProperties.PORT_DUMMY` — the external-port dummy that
    /// represents this (parent-node) port inside the nested graph.
    pub port_dummy: Option<crate::layout::elk::alg::graph::LNodeId>,
    /// `InternalProperties.INSIDE_CONNECTIONS`
    pub inside_connections: bool,
    /// `elk.port.borderOffset`
    pub port_border_offset: f64,
    /// `InternalProperties.LONG_EDGE_TARGET_NODE` — the real target node
    /// an outgoing port's edge reaches through the long-edge dummy chain
    /// (memoized by `SortByInputModelProcessor.longEdgeTargetNodePreprocessing`).
    pub long_edge_target_node: Option<crate::layout::elk::alg::graph::LNodeId>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeProps {
    /// `InternalProperties.ORIGIN`
    pub origin: Option<String>,
    /// `InternalProperties.REVERSED` — flipped by every
    /// `LEdge::reverse` call (cycle breaking, compound processing).
    pub reversed: bool,
    /// `elk.priority.direction` (cycle breaking weights edges by it)
    pub priority: i32,
    /// `InternalProperties.MODEL_ORDER`
    pub model_order: Option<i32>,
    /// `InternalProperties.ORIGINAL_OPPOSITE_PORT` — set by
    /// `LayerConstraintPreprocessor` when it hides a *_SEPARATE node: the
    /// port on the far end of a disconnected edge, so the postprocessor
    /// can reconnect it.
    pub original_opposite_port: Option<crate::layout::elk::alg::graph::LPortId>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LabelProps {
    /// `elk.edgeLabels.placement`: head/tail swap on reversal.
    pub placement: EdgeLabelPlacement,
    /// `elk.nodeLabels.placement` raw option value for node labels
    /// (only `OUTSIDE V_BOTTOM H_CENTER` occurs in scope).
    pub node_placement: Option<String>,
    /// `elk.edgeLabels.inline` **on the label element**. draw-uml sets
    /// the option on the *edge*, which ELK's importer does not propagate
    /// to labels — elkjs-verified no-op — so this stays false in scope.
    pub inline: bool,
    /// `InternalProperties.ORIGINAL_LABEL_EDGE` — the cross-hierarchy
    /// edge this label was moved off of by the compound preprocessor.
    pub original_label_edge: Option<crate::layout::elk::alg::graph::LEdgeId>,
}

/// `org.eclipse.elk.core.options.EdgeLabelPlacement`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EdgeLabelPlacement {
    #[default]
    Center,
    Head,
    Tail,
}
