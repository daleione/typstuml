//! The graph the Sugiyama placer operates on.
//!
//! `VisualGraph` stores a flat list of `Element`s (sized boxes — the actual
//! visual is rendered Typst-side, this layer only computes coordinates) and
//! an edge list. Edges carry a single `src_row` index that the caller uses
//! to identify which row of a multi-row record an edge originates from;
//! the placer itself doesn't read it.
//!
//! Connectors are zero-size dummy nodes that the lowering step inserts to
//! break edges spanning more than one rank, so each rendered edge segment
//! goes between adjacent ranks. They survive into `iter_edges`.

use std::mem::swap;

use crate::layout::dag::{NodeHandle, NodeIterator, DAG};
use crate::layout::geometry::{
    box_edge_intersection, passthrough_control_point, Point, Position,
};

#[derive(Debug, Clone, Copy)]
pub enum Orientation {
    TopToBottom,
    LeftToRight,
}

impl Orientation {
    pub fn is_top_to_bottom(&self) -> bool {
        matches!(self, Orientation::TopToBottom)
    }
    pub fn is_left_right(&self) -> bool {
        matches!(self, Orientation::LeftToRight)
    }
    pub fn flip(&self) -> Orientation {
        match self {
            Orientation::TopToBottom => Orientation::LeftToRight,
            Orientation::LeftToRight => Orientation::TopToBottom,
        }
    }
}

/// Halo around each `Element`, used as the rank-step and minimum sibling
/// gap by `simple::assign_y_coordinates` and `BK::first_schedule_x`. In
/// Typst pt — set to give visually breathing room around 10pt records.
const NODE_HALO: f64 = 20.;

/// Halo around connector dummy nodes. Smaller than `NODE_HALO` so a long
/// edge passing through several ranks doesn't push them apart.
const CONNECTOR_HALO: f64 = 10.;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeKind {
    Box,
    Connector,
}

#[derive(Debug, Clone)]
pub struct Element {
    pub kind: NodeKind,
    pub pos: Position,
    pub orientation: Orientation,
}

impl Element {
    pub fn new_box(size: Point, orientation: Orientation) -> Element {
        Element {
            kind: NodeKind::Box,
            pos: Position::new(
                Point::zero(),
                size,
                Point::zero(),
                Point::splat(NODE_HALO),
            ),
            orientation,
        }
    }

    pub fn new_connector(orientation: Orientation) -> Element {
        Element {
            kind: NodeKind::Connector,
            pos: Position::new(
                Point::zero(),
                Point::zero(),
                Point::zero(),
                Point::splat(CONNECTOR_HALO),
            ),
            orientation,
        }
    }

    pub fn is_connector(&self) -> bool {
        matches!(self.kind, NodeKind::Connector)
    }

    pub fn position(&self) -> Position {
        self.pos
    }

    pub fn position_mut(&mut self) -> &mut Position {
        &mut self.pos
    }

    pub fn move_to(&mut self, to: Point) {
        self.pos.move_to(to);
    }

    pub fn transpose(&mut self) {
        self.orientation = self.orientation.flip();
        self.pos.transpose();
    }

    /// Connectors have a zero footprint that the placer queries via
    /// `Position::size`; recompute it so it sits on the inbound rank step.
    pub fn resize(&mut self) {
        if !self.is_connector() {
            return;
        }
        let size = Point::new(1., 1.);
        self.pos.set_size(size);
        let center = match self.orientation {
            Orientation::TopToBottom => Point::new(0., size.y / 2.),
            Orientation::LeftToRight => Point::new(size.x / 2., 0.),
        };
        self.pos.set_new_center_point(center);
    }

    /// Where an edge coming from `from` lands on this node, plus the
    /// outbound bezier control point. Used by `curve::generate_curve`.
    pub fn connector_location(&self, from: Point, force: f64) -> (Point, Point) {
        box_edge_intersection(self.pos.center(), self.pos.size(false), from, force)
    }

    /// Bezier control point for an edge passing through this connector,
    /// coming from `from` and heading to `to`.
    pub fn passthrough_control(
        &self,
        from: Point,
        to: Point,
        force: f64,
    ) -> (Point, Point) {
        debug_assert!(
            self.is_connector(),
            "passthrough only defined for connectors"
        );
        passthrough_control_point(self.pos.center(), from, to, force)
    }
}

/// Per-edge metadata. The placer doesn't use it; only the caller does
/// (e.g. record-graph emit-time, to know which row an edge leaves from).
#[derive(Debug, Clone, Copy, Default)]
pub struct Edge {
    pub src_row: usize,
}

#[derive(Debug)]
pub struct VisualGraph {
    nodes: Vec<Element>,
    edges: Vec<(Edge, Vec<NodeHandle>)>,
    self_edges: Vec<(Edge, NodeHandle)>,
    pub dag: DAG,
    orientation: Orientation,
}

impl VisualGraph {
    pub fn new(orientation: Orientation) -> Self {
        VisualGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            self_edges: Vec::new(),
            dag: DAG::new(),
            orientation,
        }
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    pub fn num_nodes(&self) -> usize {
        self.dag.len()
    }

    pub fn iter_nodes(&self) -> NodeIterator {
        self.dag.iter()
    }

    pub fn succ(&self, node: NodeHandle) -> &Vec<NodeHandle> {
        self.dag.successors(node)
    }

    pub fn preds(&self, node: NodeHandle) -> &Vec<NodeHandle> {
        self.dag.predecessors(node)
    }

    pub fn pos(&self, n: NodeHandle) -> Position {
        self.element(n).position()
    }

    pub fn pos_mut(&mut self, n: NodeHandle) -> &mut Position {
        self.element_mut(n).position_mut()
    }

    pub fn is_connector(&self, n: NodeHandle) -> bool {
        self.element(n).is_connector()
    }

    pub fn transpose(&mut self) {
        for node in self.dag.iter() {
            self.element_mut(node).transpose();
        }
    }

    pub fn element(&self, node: NodeHandle) -> &Element {
        &self.nodes[node.get_index()]
    }

    pub fn element_mut(&mut self, node: NodeHandle) -> &mut Element {
        &mut self.nodes[node.get_index()]
    }

    pub fn add_node(&mut self, elem: Element) -> NodeHandle {
        let h = self.dag.new_node();
        debug_assert_eq!(h.get_index(), self.nodes.len());
        self.nodes.push(elem);
        h
    }

    pub fn add_edge(&mut self, edge: Edge, from: NodeHandle, to: NodeHandle) {
        debug_assert!(from.get_index() < self.nodes.len());
        debug_assert!(to.get_index() < self.nodes.len());
        self.edges.push((edge, vec![from, to]));
    }

    /// Edges in lowered form: `(edge, [from, ..connectors, to])`.
    pub fn iter_edges(&self) -> impl Iterator<Item = (&Edge, &[NodeHandle])> {
        self.edges.iter().map(|(e, l)| (e, l.as_slice()))
    }

    /// Run lowering and placement. After this, `pos(node)` is the final
    /// coordinate and `iter_edges` walks each edge with its inserted
    /// connector chain.
    pub fn layout(&mut self) {
        self.lower();
        crate::layout::sugiyama::Placer::new(self).run();
    }

    fn lower(&mut self) {
        self.normalize_dag();
        self.split_long_edges();
        for elem in self.dag.iter() {
            self.element_mut(elem).resize();
        }
    }

    /// Reverse back-edges (so the DAG actually is acyclic) and stash any
    /// self-edges for later expansion.
    fn normalize_dag(&mut self) {
        let edges = std::mem::take(&mut self.edges);
        debug_assert_eq!(self.nodes.len(), self.dag.len(), "node/dag size mismatch");

        for (edge, mut lst) in edges {
            debug_assert_eq!(lst.len(), 2);
            let mut from = lst[0];
            let mut to = lst[1];

            if from == to {
                self.self_edges.push((edge, from));
                continue;
            }
            if self.dag.is_reachable(to, from) {
                swap(&mut from, &mut to);
            }
            self.dag.add_edge(from, to);
            lst[0] = from;
            lst[1] = to;
            self.edges.push((edge, lst));
            self.dag.verify();
        }
    }

    /// Insert connector dummies so each remaining edge spans exactly one
    /// rank, then run the rank/edge-cross optimizers and expand self-edges.
    fn split_long_edges(&mut self) {
        self.dag.recompute_node_ranks();
        self.dag.verify();
        crate::layout::sugiyama::RankOptimizer::new(&mut self.dag).optimize();

        let mut edges = std::mem::take(&mut self.edges);
        for (_, lst) in edges.iter_mut() {
            let mut i = 1;
            while i < lst.len() {
                let prev = lst[i - 1];
                let curr = lst[i];
                let prev_level = self.dag.level(prev);
                let curr_level = self.dag.level(curr);
                debug_assert!(prev_level < curr_level, "invalid edge");
                if prev_level + 1 == curr_level {
                    i += 1;
                    continue;
                }
                let dir = self.element(prev).orientation;
                let conn = self.add_node(Element::new_connector(dir));
                lst.insert(i, conn);
                self.dag.remove_edge(prev, curr);
                self.dag.add_edge(prev, conn);
                self.dag.add_edge(conn, curr);
                self.dag.update_node_rank_level(conn, prev_level + 1, None);
            }
        }
        self.edges = edges;

        crate::layout::sugiyama::EdgeCrossOptimizer::new(&mut self.dag).optimize();
        self.expand_self_edges();
    }

    fn expand_self_edges(&mut self) {
        for (edge, node) in std::mem::take(&mut self.self_edges) {
            let level = self.dag.level(node);
            let dir = self.element(node).orientation;
            let conn = self.add_node(Element::new_connector(dir));
            self.dag.update_node_rank_level(conn, level, Some(node));
            self.edges.push((edge, vec![node, conn, node]));
        }
    }
}
