//! Port of ELK's post-compaction for the LEFT strategy: the layered-side
//! `HorizontalGraphCompactor` + `LGraphToCGraphTransformer` +
//! `VerticalSegment` (`org.eclipse.elk.alg.layered.intermediate.compaction`)
//! over the one-dimensional compactor `OneDimensionalCompactor` +
//! `LongestPathCompaction` + `ScanlineConstraintCalculator` /
//! `EdgeAwareScanlineConstraintCalculation`
//! (`org.eclipse.elk.alg.common.compaction.oned`). EPL-2.0 (see
//! `LICENSE.md`).
//!
//! Scope (asserted at the option parse): strategy LEFT only — no
//! direction changes, so hitboxes are never mirrored/transposed and
//! constraints never reversed; constraint calculation SCANLINE (the
//! option's default); orthogonal edge routing; no comment boxes, no
//! self-loops, no splines, no junction points, no north/south ports
//! (group ports normalize to east/west — the literal N/S branches of the
//! transformer are unreachable) and no lock functions (LEFT never locks).
//!
//! The compactor moves nodes and vertical edge segments along the layer
//! axis (internal x), then rewrites the graph's offset and size from the
//! compacted bounding box and re-pins external-port dummies to it.

use std::collections::HashMap;

use super::graph::{LEdgeId, LGraphArena, LGraphId, LNodeId, NodeType};
use super::math::KVector;
use super::spacings;

const TOLERANCE: f64 = 0.0001;
/// `EdgeAwareScanlineConstraintCalculation.EPSILON`.
const EPSILON: f64 = 0.5;
/// `EdgeAwareScanlineConstraintCalculation.SMALL_EPSILON`.
const SMALL_EPSILON: f64 = 0.01;

fn fuzzy_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE
}
fn fuzzy_cmp(a: f64, b: f64) -> std::cmp::Ordering {
    if (a - b).abs() <= TOLERANCE {
        std::cmp::Ordering::Equal
    } else {
        a.total_cmp(&b)
    }
}
fn fuzzy_lt(a: f64, b: f64) -> bool {
    fuzzy_cmp(a, b) == std::cmp::Ordering::Less
}

#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// What a `CNode` stands for (Java `CNode.origin`).
enum Origin {
    Node(LNodeId),
    Segment(VerticalSegment),
}

/// Java `VerticalSegment` (orthogonal subset: no bounding boxes, no
/// junction points, no pre-computed constraints, no ports).
struct VerticalSegment {
    /// (edge, bend index) pairs whose points sit in this segment —
    /// Java's `affectedBends` object references.
    affected_bends: Vec<(LEdgeId, usize)>,
    represented_ledges: Vec<LEdgeId>,
    hitbox: Rect,
    ignore_spacing_up: bool,
    ignore_spacing_down: bool,
}

impl VerticalSegment {
    fn new(bend1: KVector, bend2: KVector, bends: Vec<(LEdgeId, usize)>, edge: LEdgeId) -> Self {
        VerticalSegment {
            affected_bends: bends,
            represented_ledges: vec![edge],
            hitbox: Rect {
                x: bend1.x.min(bend2.x),
                y: bend1.y.min(bend2.y),
                width: (bend1.x - bend2.x).abs(),
                height: (bend1.y - bend2.y).abs(),
            },
            ignore_spacing_up: false,
            ignore_spacing_down: false,
        }
    }

    /// Java `joinWith` (orthogonal subset).
    fn join_with(&mut self, other: VerticalSegment) {
        self.represented_ledges.extend(other.represented_ledges);
        self.affected_bends.extend(other.affected_bends);
        let new_x = self.hitbox.x.min(other.hitbox.x);
        let new_y = self.hitbox.y.min(other.hitbox.y);
        let max_x = (self.hitbox.x + self.hitbox.width).max(other.hitbox.x + other.hitbox.width);
        let max_y = (self.hitbox.y + self.hitbox.height).max(other.hitbox.y + other.hitbox.height);
        self.hitbox = Rect { x: new_x, y: new_y, width: max_x - new_x, height: max_y - new_y };
        self.ignore_spacing_up |= other.ignore_spacing_up;
        self.ignore_spacing_down |= other.ignore_spacing_down;
    }

    /// Java `intersects`: same fuzzy x and overlapping y ranges.
    fn intersects(&self, o: &VerticalSegment) -> bool {
        fuzzy_eq(self.hitbox.x, o.hitbox.x)
            && !(fuzzy_lt(self.hitbox.y + self.hitbox.height, o.hitbox.y)
                || fuzzy_lt(o.hitbox.y + o.hitbox.height, self.hitbox.y))
    }
}

/// Java `CNode` (+ its singleton `CGroup`, folded in: in scope no
/// north/south segments exist, so every group ends up with exactly one
/// member and Java's group machinery degenerates to per-node state).
struct CNode {
    origin: Origin,
    hitbox: Rect,
    hitbox_pre_compaction: Rect,
    /// Constraints: CNodes to this one's right that it pushes.
    constraints: Vec<usize>,
    start_pos: f64,
    /// Group state (singleton): remaining / real out-degrees.
    out_degree: usize,
    group_start_pos: f64,
}

/// Run the LEFT post-compaction on a laid-out graph, exactly at the
/// `HORIZONTAL_COMPACTOR` slot (after `LongEdgeJoiner`, before
/// `ReversedEdgeRestorer`).
pub fn horizontal_graph_compactor_left(arena: &mut LGraphArena, graph: LGraphId) {
    let mut cnodes: Vec<CNode> = Vec::new();
    let mut node_cnode: HashMap<LNodeId, usize> = HashMap::new();

    // ---- LGraphToCGraphTransformer.transformNodes --------------------
    for layer in &arena.graphs[graph.0].layers {
        for &node in &layer.nodes {
            let n = &arena.nodes[node.0];
            let hitbox = Rect {
                x: n.position.x - n.margin.left,
                y: n.position.y - n.margin.top,
                width: n.size.x + n.margin.left + n.margin.right,
                height: n.size.y + n.margin.top + n.margin.bottom,
            };
            node_cnode.insert(node, cnodes.len());
            cnodes.push(CNode {
                origin: Origin::Node(node),
                hitbox,
                hitbox_pre_compaction: hitbox,
                constraints: Vec::new(),
                start_pos: f64::NEG_INFINITY,
                out_degree: 0,
                group_start_pos: f64::NEG_INFINITY,
            });
        }
    }

    // ---- collectVerticalSegmentsOrthogonal ---------------------------
    let mut segments: Vec<VerticalSegment> = Vec::new();
    for layer in &arena.graphs[graph.0].layers {
        for &node in &layer.nodes {
            let cnode_box = cnodes[node_cnode[&node]].hitbox;
            for edge in arena.node_outgoing_edges(node) {
                let bends = arena.edges[edge.0].bend_points.clone();
                if bends.is_empty() {
                    continue;
                }
                // Source/target N/S-port segments never occur (normalized
                // east/west frame); asserted by absence of N/S real ports.
                let mut bend1 = bends[0];
                let mut i1 = 0usize;
                let mut first = true;
                let mut last_segment: Option<usize> = None;
                let mut it = 1usize;
                while it < bends.len() {
                    let bend2 = bends[it];
                    if !fuzzy_eq(bend1.y, bend2.y) {
                        let vs = VerticalSegment::new(
                            bend1,
                            bend2,
                            vec![(edge, i1), (edge, it)],
                            edge,
                        );
                        segments.push(vs);
                        last_segment = Some(segments.len() - 1);
                        if first {
                            first = false;
                            let s = segments.last_mut().unwrap();
                            if bend2.y < cnode_box.y {
                                s.ignore_spacing_down = true;
                            } else if bend2.y > cnode_box.y + cnode_box.height {
                                s.ignore_spacing_up = true;
                            } else {
                                s.ignore_spacing_up = true;
                                s.ignore_spacing_down = true;
                            }
                        }
                    }
                    if it + 1 < bends.len() {
                        bend1 = bends[it];
                        i1 = it;
                    }
                    it += 1;
                }
                if let Some(ls) = last_segment {
                    let target_node = arena.edge_target_node(edge).unwrap();
                    let target_box = cnodes[node_cnode[&target_node]].hitbox;
                    let s = &mut segments[ls];
                    if bend1.y < target_box.y {
                        s.ignore_spacing_down = true;
                    } else if bend1.y > target_box.y + target_box.height {
                        s.ignore_spacing_up = true;
                    } else {
                        s.ignore_spacing_up = true;
                        s.ignore_spacing_down = true;
                    }
                }
            }
        }
    }

    // ---- mergeVerticalSegments ---------------------------------------
    segments.sort_by(|a, b| {
        fuzzy_cmp(a.hitbox.x, b.hitbox.x).then_with(|| a.hitbox.y.total_cmp(&b.hitbox.y))
    });
    let mut merged: Vec<VerticalSegment> = Vec::new();
    for next in segments {
        match merged.last_mut() {
            Some(survivor) if survivor.intersects(&next) => survivor.join_with(next),
            _ => merged.push(next),
        }
    }
    for vs in merged {
        cnodes.push(CNode {
            hitbox: vs.hitbox,
            hitbox_pre_compaction: vs.hitbox,
            origin: Origin::Segment(vs),
            constraints: Vec::new(),
            start_pos: f64::NEG_INFINITY,
            out_degree: 0,
            group_start_pos: f64::NEG_INFINITY,
        });
    }

    // ---- EdgeAwareScanlineConstraintCalculation (orthogonal) ---------
    let spacing = arena.graphs[graph.0].props.spacing;
    let vertical_edge_edge_spacing = spacing.edge_edge;
    let is_ext_port = |arena: &LGraphArena, c: &CNode| match c.origin {
        Origin::Node(n) => arena.nodes[n.0].node_type == NodeType::ExternalPort,
        Origin::Segment(_) => false,
    };

    // Phase 1: vertical segments only.
    let spacing_vs = 0.0f64.max(vertical_edge_edge_spacing / 2.0 - EPSILON);
    alter_segment_hitboxes(&mut cnodes, spacing_vs, 1.0);
    sweep(arena, &mut cnodes, |c| matches!(c.origin, Origin::Segment(_)));
    alter_segment_hitboxes(&mut cnodes, spacing_vs, -1.0);

    // Phase 2: nodes only (edge-edge spacing on purpose, see Java note).
    let delta_node = 0.0f64.max(spacing.edge_edge / 2.0 - EPSILON);
    for c in cnodes.iter_mut() {
        if matches!(c.origin, Origin::Node(_)) {
            c.hitbox.y -= delta_node;
            c.hitbox.height += 2.0 * delta_node;
        }
    }
    sweep(arena, &mut cnodes, |c| matches!(c.origin, Origin::Node(_)));
    for c in cnodes.iter_mut() {
        if matches!(c.origin, Origin::Node(_)) {
            c.hitbox.y += delta_node;
            c.hitbox.height -= 2.0 * delta_node;
        }
    }

    // Phase 3: everything, grown by the graph-wide minimum spacing.
    let mut min_spacing = f64::INFINITY;
    for c in &cnodes {
        let v = if is_ext_port(arena, c) {
            f64::INFINITY
        } else {
            match c.origin {
                Origin::Segment(_) => 0.0f64.max(vertical_edge_edge_spacing / 2.0 - EPSILON),
                Origin::Node(_) => 0.0f64.max(spacing.node_node / 2.0 - EPSILON),
            }
        };
        min_spacing = min_spacing.min(v);
    }
    if !min_spacing.is_finite() {
        min_spacing = 0.0;
    }
    // Singleton groups: `alterGroupedHitboxOrthogonal` reduces to
    // altering the group's only member like `alterHitbox`.
    alter_all_hitboxes(&mut cnodes, min_spacing, 1.0);
    sweep(arena, &mut cnodes, |_| true);
    alter_all_hitboxes(&mut cnodes, min_spacing, -1.0);

    // ---- OneDimensionalCompactor.compact + LongestPathCompaction -----
    // Group out-degrees: each constraint into a (singleton) group.
    for i in 0..cnodes.len() {
        for k in 0..cnodes[i].constraints.len() {
            let target = cnodes[i].constraints[k];
            cnodes[target].out_degree += 1;
        }
    }

    let mut min_start_pos = f64::INFINITY;
    for c in &cnodes {
        min_start_pos = min_start_pos.min(c.hitbox.x);
    }

    let mut sinks: std::collections::VecDeque<usize> = Default::default();
    for (i, c) in cnodes.iter_mut().enumerate() {
        c.group_start_pos = min_start_pos;
        if c.out_degree == 0 {
            sinks.push_back(i);
        }
    }
    while let Some(gi) = sinks.pop_front() {
        cnodes[gi].start_pos = cnodes[gi].group_start_pos;
        for k in 0..cnodes[gi].constraints.len() {
            let inc = cnodes[gi].constraints[k];
            let spacing_h = horizontal_spacing_handler(arena, &cnodes, &spacing, gi, inc);
            let pushed = cnodes[gi].start_pos + cnodes[gi].hitbox.width + spacing_h;
            cnodes[inc].group_start_pos = cnodes[inc].group_start_pos.max(pushed);
            cnodes[inc].out_degree -= 1;
            if cnodes[inc].out_degree == 0 {
                sinks.push_back(inc);
            }
        }
    }
    for c in cnodes.iter_mut() {
        c.hitbox.x = c.start_pos;
    }

    // ---- LGraphToCGraphTransformer.applyLayout ------------------------
    for c in &cnodes {
        if let Origin::Node(node) = c.origin {
            arena.nodes[node.0].position.x = c.hitbox.x + arena.nodes[node.0].margin.left;
        }
    }
    for c in &cnodes {
        if let Origin::Segment(vs) = &c.origin {
            let delta_x = c.hitbox.x - c.hitbox_pre_compaction.x;
            for &(edge, idx) in &vs.affected_bends {
                arena.edges[edge.0].bend_points[idx].x += delta_x;
            }
        }
    }

    let mut top_left = KVector::new(f64::INFINITY, f64::INFINITY);
    let mut bottom_right = KVector::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for c in &cnodes {
        top_left.x = top_left.x.min(c.hitbox.x);
        top_left.y = top_left.y.min(c.hitbox.y);
        bottom_right.x = bottom_right.x.max(c.hitbox.x + c.hitbox.width);
        bottom_right.y = bottom_right.y.max(c.hitbox.y + c.hitbox.height);
    }
    arena.graphs[graph.0].offset = KVector::new(-top_left.x, -top_left.y);
    arena.graphs[graph.0].size =
        KVector::new(bottom_right.x - top_left.x, bottom_right.y - top_left.y);

    // applyExternalPortPositions (normalized east/west frame).
    for c in &cnodes {
        if let Origin::Node(node) = c.origin {
            if arena.nodes[node.0].node_type != NodeType::ExternalPort {
                continue;
            }
            let side = match arena.nodes[node.0].props.ext_port_side {
                super::options::PortSide::North => super::options::PortSide::West,
                super::options::PortSide::South => super::options::PortSide::East,
                s => s,
            };
            match side {
                super::options::PortSide::West => {
                    arena.nodes[node.0].position.x = top_left.x;
                }
                super::options::PortSide::East => {
                    arena.nodes[node.0].position.x = bottom_right.x
                        - (arena.nodes[node.0].size.x + arena.nodes[node.0].margin.right);
                }
                _ => panic!("north/south external ports are outside the ported scope"),
            }
        }
    }
}

/// Java `alterHitbox` for one CNode: `delta = spacing * fac`, and — kept
/// bug-for-bug — `SMALL_EPSILON` is *not* multiplied by `fac`, so an
/// apply/undo pair does not restore the segment hitbox exactly (it ends
/// up 0.01 higher and 0.02 taller, as in ELK).
fn alter_hitbox(c: &mut CNode, spacing: f64, fac: f64) {
    let delta = spacing * fac;
    match &c.origin {
        Origin::Segment(vs) => {
            if !vs.ignore_spacing_up {
                c.hitbox.y -= delta + SMALL_EPSILON;
                c.hitbox.height += delta + SMALL_EPSILON;
            } else if !vs.ignore_spacing_down {
                c.hitbox.height += delta + SMALL_EPSILON;
            }
        }
        Origin::Node(_) => {
            c.hitbox.y -= delta;
            c.hitbox.height += 2.0 * delta;
        }
    }
}

/// Phase-1 helper: `alterHitbox` on vertical segments only.
fn alter_segment_hitboxes(cnodes: &mut [CNode], spacing: f64, fac: f64) {
    for c in cnodes.iter_mut() {
        if matches!(c.origin, Origin::Segment(_)) {
            alter_hitbox(c, spacing, fac);
        }
    }
}

/// Phase-3 `alterGroupedHitboxOrthogonal` over singleton groups =
/// `alterHitbox` on each group's only member.
fn alter_all_hitboxes(cnodes: &mut [CNode], spacing: f64, fac: f64) {
    for c in cnodes.iter_mut() {
        alter_hitbox(c, spacing, fac);
    }
}

/// `HorizontalGraphCompactor.specialSpacingsHandler.getHorizontalSpacing`.
fn horizontal_spacing_handler(
    arena: &LGraphArena,
    cnodes: &[CNode],
    spacing: &super::options::SpacingProps,
    a: usize,
    b: usize,
) -> f64 {
    // Vertical segments of the same edge join at spacing 0.
    if let (Origin::Segment(v1), Origin::Segment(v2)) = (&cnodes[a].origin, &cnodes[b].origin) {
        if v1.represented_ledges.iter().any(|e| v2.represented_ledges.contains(e)) {
            return 0.0;
        }
    }
    let lnode = |c: &CNode| match c.origin {
        Origin::Node(n) => Some(n),
        Origin::Segment(_) => None,
    };
    let (n1, n2) = (lnode(&cnodes[a]), lnode(&cnodes[b]));
    // External ports may move arbitrarily close; they are re-pinned later.
    for n in [n1, n2].into_iter().flatten() {
        if arena.nodes[n.0].node_type == NodeType::ExternalPort {
            return 0.0;
        }
    }
    let ty = |n: Option<LNodeId>| match n {
        Some(n) => arena.nodes[n.0].node_type,
        None => NodeType::LongEdge,
    };
    spacings::horizontal_spacing(spacing, ty(n1), ty(n2))
}

// ----------------------------------------------------------------------
// ScanlineConstraintCalculator
// ----------------------------------------------------------------------

/// One scanline pass over the filtered nodes, adding constraints (Java
/// `ScanlineConstraintCalculator.sweep`). The interval set is ordered by
/// hitbox center-x; `cand` remembers each node's left-neighbor candidate.
fn sweep(arena: &LGraphArena, cnodes: &mut [CNode], filter: impl Fn(&CNode) -> bool) {
    let _ = arena;
    #[derive(Clone, Copy)]
    struct Timestamp {
        node: usize,
        low: bool,
    }
    let mut points: Vec<Timestamp> = Vec::new();
    for (i, c) in cnodes.iter().enumerate() {
        if filter(c) {
            points.push(Timestamp { node: i, low: true });
            points.push(Timestamp { node: i, low: false });
        }
    }
    let y_of = |cnodes: &[CNode], p: &Timestamp| {
        let b = cnodes[p.node].hitbox;
        if p.low { b.y } else { b.y + b.height }
    };
    points.sort_by(|p1, p2| {
        let c = y_of(cnodes, p1).total_cmp(&y_of(cnodes, p2));
        if c == std::cmp::Ordering::Equal {
            // equal coordinates: high (closing) before low (opening)
            match (p1.low, p2.low) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        } else {
            c
        }
    });

    // Ordered active set by hitbox center x (Java TreeSet semantics:
    // center equality would be a duplicate → invalid hitboxes).
    let center = |cnodes: &[CNode], i: usize| cnodes[i].hitbox.x + cnodes[i].hitbox.width / 2.0;
    let mut intervals: Vec<usize> = Vec::new();
    let mut cand: Vec<Option<usize>> = vec![None; cnodes.len()];

    for p in points {
        let c = center(cnodes, p.node);
        if p.low {
            // insert
            let pos = intervals
                .binary_search_by(|&i| center(cnodes, i).total_cmp(&c))
                .unwrap_or_else(|e| e);
            debug_assert!(
                intervals.get(pos).is_none_or(|&i| center(cnodes, i) != c),
                "invalid hitboxes for scanline constraint calculation"
            );
            intervals.insert(pos, p.node);
            cand[p.node] = pos.checked_sub(1).map(|q| intervals[q]);
            if let Some(&right) = intervals.get(pos + 1) {
                cand[right] = Some(p.node);
            }
        } else {
            // delete
            let pos = intervals.iter().position(|&i| i == p.node).unwrap();
            if let Some(left) = pos.checked_sub(1).map(|q| intervals[q]) {
                if cand[p.node] == Some(left) {
                    // singleton groups: distinct nodes = distinct groups
                    cnodes[left].constraints.push(p.node);
                }
            }
            if let Some(&right) = intervals.get(pos + 1) {
                if cand[right] == Some(p.node) {
                    cnodes[p.node].constraints.push(right);
                }
            }
            intervals.remove(pos);
        }
    }
}
