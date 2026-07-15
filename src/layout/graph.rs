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
use crate::layout::geometry::{Point, Position};
use crate::layout::spacing::Spacing;

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

/// Gap between adjacent ranks (parallel to the rank-progression axis).
/// PlantUML/dot defaults to `ranksep ≈ 36pt` at a 14pt body; for our 10pt
/// body we use a tighter value, since `compact::do_it` further reclaims
/// any rank-gap that no actual neighbour needs.
const RANK_GAP_PT: f64 = 12.;

/// Gap between sibling boxes within the same rank (perpendicular to the
/// rank-progression axis). Half the rank gap, mirroring dot's
/// `nodesep ≈ ranksep / 2` ratio.
const SIBLING_GAP_PT: f64 = 6.;

/// Halo for connector dummies — symmetric and small so a multi-rank edge
/// doesn't bow neighboring records apart.
const CONNECTOR_HALO_PT: f64 = SIBLING_GAP_PT;

/// Halo a real box gets, given the rank-progression axis. Halo is added to
/// `size` and split symmetrically around the box, so the actual gap between
/// neighbors equals one full halo (see `simple::assign_y_coordinates` and
/// `BK::first_schedule_x`).
fn box_halo(orientation: Orientation) -> Point {
    if orientation.is_left_right() {
        Point::new(RANK_GAP_PT, SIBLING_GAP_PT)
    } else {
        Point::new(SIBLING_GAP_PT, RANK_GAP_PT)
    }
}

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
            pos: Position::new(Point::zero(), size, Point::zero(), box_halo(orientation)),
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
                Point::splat(CONNECTOR_HALO_PT),
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
}

/// Per-edge metadata. `src_row` is consumed at emit time; the optional
/// `*_perp_offset` fields are read by `port_align` to bias child nodes
/// toward the parent row that references them.
///
/// `perp_offset` semantics are orientation-invariant: a length along the
/// axis perpendicular to rank progression, measured from the box's
/// top/left edge. It is *not* transposed when the placer flips the
/// graph — the field is a length, not a coordinate.
#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub src_row: usize,
    pub source_perp_offset: Option<f64>,
    pub target_perp_offset: Option<f64>,
    /// Minimum rank span (dot's `minlen`). `head` sits at least this many
    /// ranks below `tail`. Only consulted by the network-simplex ranking
    /// path (`ns_rank`); the longest-path path treats every edge as 1.
    pub min_rank: usize,
}

impl Default for Edge {
    fn default() -> Self {
        Edge {
            src_row: 0,
            source_perp_offset: None,
            target_perp_offset: None,
            min_rank: 1,
        }
    }
}

#[derive(Debug)]
pub struct VisualGraph {
    nodes: Vec<Element>,
    edges: Vec<(Edge, Vec<NodeHandle>)>,
    self_edges: Vec<(Edge, NodeHandle)>,
    pub dag: DAG,
    orientation: Orientation,
    /// Optional cluster annotation. When present, the Sugiyama passes
    /// keep each cluster's members contiguous in row order and inside a
    /// shared x-extent. Empty by default → original flat behaviour.
    pub hierarchy: crate::layout::sugiyama::HierarchyMap,
    /// When set, x-coordinates are assigned by dot's network-simplex
    /// method instead of Brandes-Köpf (and the BK companion passes
    /// port_align / edge_fix are skipped). Enabled for dot-style state
    /// diagrams; record graphs keep BK + port alignment.
    pub ns_xcoord: bool,
    /// When set, rank (y) assignment uses network simplex honouring each
    /// edge's `min_rank` (dot's minlen) instead of longest-path + sinking.
    pub ns_rank: bool,
    /// Font-scaled spacing table consulted by cuca's compound layout,
    /// `hierarchy::apply_cluster_margins`, and `tighten`'s sibling /
    /// stranger separation. Defaults to the pre-M2 constants
    /// ([`Spacing::legacy`]) so every non-cuca diagram family is
    /// unaffected; cuca opts in via `set_spacing`.
    pub spacing: Spacing,
    /// When set, `EdgeCrossOptimizer` prefers orderings closer to the
    /// diagram's declared order among crossing-count ties, and skips
    /// the row rotation/perturbation that would otherwise scramble it
    /// for no benefit (§3.7). Off by default; cuca opts in via
    /// `set_model_order`.
    pub model_order: bool,
    /// When set, rank assignment uses compound-graph (per-cluster)
    /// critical-path ranking instead of flat longest-path — every
    /// cluster's own height reflects only its own content, instead of
    /// being stretched across whatever global rank numbers unrelated
    /// clusters happen to be using (M7's real aspect-ratio fix; see
    /// `sugiyama::cluster_rank`). Takes priority over `ns_rank` when
    /// both are set (cuca never sets `ns_rank`, so this doesn't arise
    /// in practice). Off by default; cuca opts in via
    /// `set_cluster_rank`.
    pub cluster_rank: bool,
}

impl VisualGraph {
    pub fn new(orientation: Orientation) -> Self {
        VisualGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            self_edges: Vec::new(),
            dag: DAG::new(),
            orientation,
            hierarchy: crate::layout::sugiyama::HierarchyMap::new(),
            ns_xcoord: false,
            ns_rank: false,
            spacing: Spacing::legacy(),
            model_order: false,
            cluster_rank: false,
        }
    }

    pub fn set_spacing(&mut self, spacing: Spacing) {
        self.spacing = spacing;
    }

    pub fn set_model_order(&mut self, enabled: bool) {
        self.model_order = enabled;
    }

    pub fn set_cluster_rank(&mut self, enabled: bool) {
        self.cluster_rank = enabled;
    }

    /// Use dot's network-simplex x-coordinate assignment for this graph.
    pub fn enable_ns_xcoord(&mut self) {
        self.ns_xcoord = true;
    }

    /// Use dot's network-simplex rank assignment (honouring edge
    /// `min_rank` / minlen) for this graph.
    pub fn enable_ns_rank(&mut self) {
        self.ns_rank = true;
    }

    pub fn set_hierarchy(&mut self, h: crate::layout::sugiyama::HierarchyMap) {
        self.hierarchy = h;
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

    /// Like `add_node(Element::new_box(size, orientation))` but with an
    /// explicit halo instead of the shared `box_halo` default — lets a
    /// caller (cuca) size gaps from its own `Spacing` table without
    /// changing the halo every other diagram family gets from
    /// `Element::new_box`.
    pub fn add_node_with_halo(&mut self, size: Point, halo: Point, orientation: Orientation) -> NodeHandle {
        let elem = Element {
            kind: NodeKind::Box,
            pos: Position::new(Point::zero(), size, Point::zero(), halo),
            orientation,
        };
        self.add_node(elem)
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
        if self.cluster_rank && !self.hierarchy.is_empty() {
            // Compound-graph critical-path ranking (§M7): each
            // cluster's own height reflects only its own content.
            let result =
                crate::layout::sugiyama::cluster_rank::compute(&mut self.dag, &self.hierarchy);
            // Edges the compound ranker reversed to break a
            // package-level cycle: the dag already points the layout
            // direction, so flip the endpoint lists to match — the
            // same state normalize-time back-edge reversal produces,
            // which every downstream pass already handles.
            for &(u, v) in &result.reversed {
                for (_, lst) in self.edges.iter_mut() {
                    if lst[0] == u && lst[1] == v {
                        lst.swap(0, 1);
                    }
                }
            }
            self.dag.set_node_levels(&result.ranks);
        } else if self.ns_rank {
            // dot's network-simplex rank assignment, honouring each edge's
            // min_rank (minlen). Replaces longest-path + greedy sinking.
            let n = self.dag.len();
            let ns_edges: Vec<(usize, usize, f64, f64)> = self
                .edges
                .iter()
                .map(|(e, lst)| {
                    (lst[0].get_index(), lst[1].get_index(), e.min_rank.max(1) as f64, 1.0)
                })
                .collect();
            let ranks = crate::layout::sugiyama::ns::solve(n, &ns_edges);
            let levels: Vec<usize> = ranks.iter().map(|&r| r.round().max(0.0) as usize).collect();
            self.dag.set_node_levels(&levels);
        } else {
            self.dag.recompute_node_ranks();
            self.dag.verify();
            crate::layout::sugiyama::RankOptimizer::new(&mut self.dag).optimize();
        }

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
                // Long edges spanning a cluster: connector dummies
                // inherit their source's cluster so the cluster's rank
                // span stays contiguous and cluster_bubble doesn't see
                // a "stranger" node at an interior rank.
                if !self.hierarchy.is_empty() {
                    self.hierarchy.inherit_node(prev, conn);
                }
                lst.insert(i, conn);
                self.dag.remove_edge(prev, curr);
                self.dag.add_edge(prev, conn);
                self.dag.add_edge(conn, curr);
                self.dag.update_node_rank_level(conn, prev_level + 1, None);
            }
        }
        self.edges = edges;

        // Hierarchy-aware passes: group each row by cluster (outermost
        // ancestor first) so cluster members start contiguous, then run
        // mincross with the same-cluster swap gate. Without a hierarchy
        // these are no-ops and behaviour is identical to the flat path.
        if !self.hierarchy.is_empty() {
            self.hierarchy.group_rows(&mut self.dag);
        }
        crate::layout::sugiyama::EdgeCrossOptimizer::new(&mut self.dag)
            .with_hierarchy(&self.hierarchy)
            .with_model_order(self.model_order)
            .optimize();
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
