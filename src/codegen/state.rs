//! State-diagram codegen.
//!
//! S1 scope (see `docs/state-diagram-design.md`): flat layout, no composite
//! states, no concurrent regions, no measure protocol. Pipeline:
//!
//! 1. Heuristic per-node geometry (`node_geom`) — char-count width estimate
//!    for text-bearing states, fixed sizes for pseudostates.
//! 2. Layout via `layout::graph::VisualGraph` (the same placer cuca uses).
//!    PlantUML's single-dash `A -> B` is a *horizontal* link — `A` and `B`
//!    must sit on the same rank, side by side. The Sugiyama placer only
//!    does layered (rank) layout, so we **condense** each maximal
//!    horizontal-linked component into one super-node, lay out the
//!    condensed graph (whose edges are all vertical rank edges), then
//!    expand each component back into its left-to-right members. This
//!    keeps the rank optimizer / `compact` pass happy — every condensed
//!    node is properly connected — while honoring the horizontal links.
//! 3. Emit a single `#state-layout(...)` call with absolute coordinates;
//!    the painter draws shapes + straight edges + labels.
//!
//! Self-loop transitions are kept out of the layout graph (the painter
//! draws them as an arc on the node itself) but still emitted so the
//! painter can render them.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::ir::{
    BorderStyle, Direction, LayoutDirection, LineStyle, NoteAnchor, NotePosition, RegionGroup,
    RegionOrient, StateDiagram, StateKind, StateNode, Transition,
};
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};

/// Margin reserved around the diagram content, in pt. Generous enough to
/// clear self-loop arcs and edge labels that extend past the node bboxes.
const MARGIN_PT: f64 = 30.0;
/// Heuristic average glyph advance at the 10pt body size.
const CHAR_W_PT: f64 = 6.2;
/// Heuristic glyph advance for `entry/exit/do` body rows (0.82em).
const BODY_CHAR_W_PT: f64 = 5.1;
/// Horizontal gap between adjacent members of a horizontal-linked component.
const COMP_GAP_PT: f64 = 26.0;
/// Padding between a composite state's frame and its interior content.
const COMPOSITE_PAD_PT: f64 = 16.0;
/// Height of the label band at the top of a composite state's frame.
const COMPOSITE_LABEL_BAND_PT: f64 = 22.0;
/// Extra width baked into every layout box. The Sugiyama placer's built-in
/// sibling gap (~6pt) is too tight once nodes sit side by side; inflating
/// each box and deflating the resulting top-lefts (cuca's trick) buys
/// breathing room without touching the shared placer constants.
const LAYOUT_PAD_X_PT: f64 = 14.0;
/// Extra height baked into every layout box — widens the rank gap. Edge
/// labels are drawn *beside* the edge (perpendicular nudge), not between
/// the stacked nodes, so this only has to clear the arrowhead plus a
/// little breathing room.
const LAYOUT_PAD_Y_PT: f64 = 16.0;
/// Right-side canvas space reserved for a self-loop arc + its label.
const SELF_LOOP_RESERVE_PT: f64 = 64.0;
/// Right-side canvas space reserved for a back-edge side-bow + its label.
const BACK_BOW_RESERVE_PT: f64 = 96.0;
/// Gap between an anchored note and the state it points at.
const NOTE_GAP_PT: f64 = 26.0;
/// Gap between adjacent concurrent regions inside a composite state — the
/// `--` / `||` divider line is centered in this band.
const REGION_GAP_PT: f64 = 26.0;

struct NodeGeom {
    size: Point,
}

/// Minimal union-find over node indices, used to collect maximal
/// horizontal-linked components.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        // Path compression.
        let mut cur = x;
        while self.parent[cur] != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Heuristic bounding box for one node.
fn node_geom(n: &StateNode) -> NodeGeom {
    let size = match n.kind {
        StateKind::Initial | StateKind::Final => Point::new(18.0, 18.0),
        StateKind::History | StateKind::DeepHistory => Point::new(24.0, 24.0),
        StateKind::Choice => Point::new(32.0, 32.0),
        StateKind::Fork | StateKind::Join => Point::new(70.0, 10.0),
        StateKind::SynchroBar => Point::new(60.0, 10.0),
        StateKind::Simple | StateKind::Composite => {
            let name_w = n.display.chars().count() as f64 * CHAR_W_PT + 22.0;
            if n.body.is_empty() {
                Point::new(name_w.max(56.0), 32.0)
            } else {
                let body_w = n
                    .body
                    .iter()
                    .map(|r| r.chars().count() as f64 * BODY_CHAR_W_PT + 16.0)
                    .fold(0.0_f64, f64::max);
                let w = name_w.max(body_w).max(64.0);
                let h = 26.0 + n.body.len() as f64 * 13.0 + 8.0;
                Point::new(w, h)
            }
        }
    };
    NodeGeom { size }
}

/// Estimate the rendered width (pt) of a transition's `event [guard] /
/// action` label, or `None` when the transition carries no label. Used to
/// reserve room for an interior back-edge's bow label inside its composite
/// frame. The painter renders the label at the 0.78em `_label-size`.
fn back_edge_label_width(tr: &Transition) -> Option<f64> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = tr.event.as_deref().filter(|s| !s.is_empty()) {
        parts.push(e.to_string());
    }
    if let Some(g) = tr.guard.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("[{g}]"));
    }
    if let Some(a) = tr.action.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("/ {a}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" ").chars().count() as f64 * 4.8)
}

/// Heuristic bounding box for an anchored note's yellow sticky.
fn note_geom(body: &str) -> Point {
    let lines: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.split('\n').collect()
    };
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let w = cols as f64 * BODY_CHAR_W_PT + 16.0;
    let h = lines.len() as f64 * 13.0 + 12.0;
    Point::new(w.max(44.0), h.max(24.0))
}

/// Order a component's members in horizontal-edge topological order
/// (`A -> B` places `A` before `B`). Members with no horizontal
/// constraint keep their declaration order. Falls back to declaration
/// order if the horizontal edges somehow form a cycle.
fn order_component(members: &[usize], horiz_adj: &HashMap<usize, Vec<usize>>) -> Vec<usize> {
    if members.len() <= 1 {
        return members.to_vec();
    }
    let set: HashSet<usize> = members.iter().copied().collect();
    let mut indeg: HashMap<usize, usize> = members.iter().map(|&m| (m, 0)).collect();
    for &m in members {
        if let Some(succs) = horiz_adj.get(&m) {
            for &s in succs {
                if set.contains(&s) {
                    *indeg.get_mut(&s).unwrap() += 1;
                }
            }
        }
    }
    // Kahn's algorithm — seed the queue in declaration order for stability.
    let mut queue: Vec<usize> = members.iter().copied().filter(|m| indeg[m] == 0).collect();
    let mut out: Vec<usize> = Vec::new();
    let mut qi = 0;
    while qi < queue.len() {
        let m = queue[qi];
        qi += 1;
        out.push(m);
        if let Some(succs) = horiz_adj.get(&m) {
            for &s in succs {
                if set.contains(&s) {
                    let e = indeg.get_mut(&s).unwrap();
                    *e -= 1;
                    if *e == 0 {
                        queue.push(s);
                    }
                }
            }
        }
    }
    if out.len() == members.len() {
        out
    } else {
        members.to_vec()
    }
}

/// Result of the layout pass.
struct Layout {
    /// Absolute top-left of every node, indexed like `diag.nodes`.
    top_lefts: Vec<Point>,
    /// Effective size of every node, indexed like `diag.nodes`. Equals the
    /// heuristic `node_geom` for simple states / pseudostates; for a
    /// composite state it is the computed frame size (interior bbox +
    /// padding + label band).
    eff_geom: Vec<Point>,
    /// Per-transition flag (indexed like `diag.transitions`): `true` when
    /// the transition was identified as a back-edge (it would have formed
    /// a cycle in the rank graph). The painter draws these as a side-bow
    /// instead of a straight line so they don't shoot back through the
    /// intervening states.
    back: Vec<bool>,
    /// Concurrent-region divider segments, indexed like `diag.nodes` and
    /// stored *relative to the composite frame's top-left*. Non-empty only
    /// for composite states with a `--` / `||` divider.
    dividers: Vec<Vec<(Point, Point)>>,
}

/// One laid-out level: relative positions of its direct members, the
/// level's bounding box, and the back-edge classification of each edge
/// it was handed.
struct FlatLayout {
    /// Global node index → top-left, normalized so the level starts at (0,0).
    rel: HashMap<usize, Point>,
    bbox: Point,
    /// `(transition index, is_back)` for each input edge.
    back: Vec<(usize, bool)>,
}

/// Walk `node`'s ancestor chain until a member of `set` is reached.
fn lift(mut node: usize, set: &HashSet<usize>, parent_of: &[Option<usize>]) -> Option<usize> {
    loop {
        if set.contains(&node) {
            return Some(node);
        }
        node = parent_of[node]?;
    }
}

/// Lift every transition's endpoints to their ancestor among `members`,
/// keeping only edges that connect two *distinct* members. This is how a
/// transition between deep descendants becomes a layout constraint at the
/// level that actually contains both their subtrees.
fn lift_edges(
    all_edges: &[(usize, usize, usize, bool)],
    members: &[usize],
    parent_of: &[Option<usize>],
) -> Vec<(usize, usize, usize, bool)> {
    let set: HashSet<usize> = members.iter().copied().collect();
    let mut out = Vec::new();
    for &(ti, s, d, horizontal) in all_edges {
        if let (Some(ls), Some(ld)) = (lift(s, &set, parent_of), lift(d, &set, parent_of)) {
            if ls != ld {
                out.push((ti, ls, ld, horizontal));
            }
        }
    }
    out
}

/// DFS reachability over an adjacency map — `from` reaches `to`?
fn reaches(adj: &HashMap<usize, Vec<usize>>, from: usize, to: usize) -> bool {
    if from == to {
        return true;
    }
    let mut stack = vec![from];
    let mut seen: HashSet<usize> = HashSet::from([from]);
    while let Some(x) = stack.pop() {
        if let Some(succ) = adj.get(&x) {
            for &y in succ {
                if y == to {
                    return true;
                }
                if seen.insert(y) {
                    stack.push(y);
                }
            }
        }
    }
    false
}

/// Lay out one flat level — a set of sibling `members` (global node
/// indices) with already-resolved effective geoms — via condensed-component
/// Sugiyama. `edges` are the lifted transitions among these members. See
/// the module docs for the condensed-component rationale.
fn layout_flat(
    members: &[usize],
    eff_geom: &[Point],
    edges: &[(usize, usize, usize, bool)],
    orientation: Orientation,
) -> FlatLayout {
    let m = members.len();
    if m == 0 {
        return FlatLayout {
            rel: HashMap::new(),
            bbox: Point::zero(),
            back: edges.iter().map(|&(ti, ..)| (ti, false)).collect(),
        };
    }
    // Local index space `0..m`; edges arrive in global indices.
    let local: HashMap<usize, usize> = members.iter().enumerate().map(|(l, &g)| (g, l)).collect();
    let lgeom = |l: usize| eff_geom[members[l]];
    let ledges: Vec<(usize, usize, usize, bool)> = edges
        .iter()
        .map(|&(ti, s, d, h)| (ti, local[&s], local[&d], h))
        .collect();

    // 1. Union horizontal-linked members into components.
    let mut uf = UnionFind::new(m);
    for &(_, s, d, horizontal) in &ledges {
        if horizontal {
            uf.union(s, d);
        }
    }
    let roots: Vec<usize> = (0..m).map(|i| uf.find(i)).collect();
    let mut comp_id: HashMap<usize, usize> = HashMap::new();
    let mut comp_members: Vec<Vec<usize>> = Vec::new();
    let comp_of: Vec<usize> = roots
        .iter()
        .map(|&r| {
            *comp_id.entry(r).or_insert_with(|| {
                comp_members.push(Vec::new());
                comp_members.len() - 1
            })
        })
        .collect();
    for i in 0..m {
        comp_members[comp_of[i]].push(i);
    }

    // 2. Deduped horizontal adjacency, for ordering members within a comp.
    let mut horiz_adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut horiz_seen: HashSet<(usize, usize)> = HashSet::new();
    for &(_, s, d, horizontal) in &ledges {
        if horizontal && horiz_seen.insert((s, d)) {
            horiz_adj.entry(s).or_default().push(d);
        }
    }

    // Orientation-relative axes. The Sugiyama placer stacks ranks along the
    // *rank axis* and siblings along the *perpendicular axis*: y / x for TB,
    // x / y for LR. A horizontal-linked component's members are laid out
    // along the perpendicular axis, and the inflation pad is per-axis.
    let is_lr = matches!(orientation, Orientation::LeftToRight);
    let (pad_rank, pad_perp) = (LAYOUT_PAD_Y_PT, LAYOUT_PAD_X_PT);

    // 3/4. Lay out each component internally.
    let mut member_offset: Vec<Point> = vec![Point::zero(); m];
    let mut comp_size: Vec<Point> = vec![Point::zero(); comp_members.len()];
    for (cid, cmembers) in comp_members.iter().enumerate() {
        let ordered = order_component(cmembers, &horiz_adj);
        let comp_rank = ordered
            .iter()
            .map(|&l| if is_lr { lgeom(l).x } else { lgeom(l).y })
            .fold(0.0_f64, f64::max);
        let mut cursor = 0.0_f64;
        for (k, &l) in ordered.iter().enumerate() {
            if k > 0 {
                cursor += COMP_GAP_PT;
            }
            let g = lgeom(l);
            if is_lr {
                member_offset[l] = Point::new((comp_rank - g.x) / 2.0, cursor);
                cursor += g.y;
            } else {
                member_offset[l] = Point::new(cursor, (comp_rank - g.y) / 2.0);
                cursor += g.x;
            }
        }
        comp_size[cid] = if is_lr {
            Point::new(comp_rank, cursor)
        } else {
            Point::new(cursor, comp_rank)
        };
    }

    // Perpendicular-axis center of each member within its component box.
    let member_perp: Vec<f64> = (0..m)
        .map(|l| {
            if is_lr {
                member_offset[l].y + lgeom(l).y / 2.0 + pad_perp / 2.0
            } else {
                member_offset[l].x + lgeom(l).x / 2.0 + pad_perp / 2.0
            }
        })
        .collect();

    // 5. Condensed graph — one box per component, only vertical edges.
    let mut vg = VisualGraph::new(orientation);
    let comp_handles: Vec<_> = comp_size
        .iter()
        .map(|sz| {
            let inflated = if is_lr {
                Point::new(sz.x + pad_rank, sz.y + pad_perp)
            } else {
                Point::new(sz.x + pad_perp, sz.y + pad_rank)
            };
            vg.add_node(Element::new_box(inflated, orientation))
        })
        .collect();
    let mut cond: HashMap<(usize, usize), (f64, f64, usize)> = HashMap::new();
    for &(_, s, d, horizontal) in &ledges {
        if horizontal {
            continue;
        }
        let (cs, cd) = (comp_of[s], comp_of[d]);
        if cs == cd {
            continue;
        }
        let e = cond.entry((cs, cd)).or_insert((0.0, 0.0, 0));
        e.0 += member_perp[s];
        e.1 += member_perp[d];
        e.2 += 1;
    }
    // Deterministic edge order, then a feedback-arc-set pass: an edge whose
    // target already reaches its source is a cycle (back-edge) — keep it
    // out of the rank graph so the placer doesn't reverse it into a
    // rank-skipping long edge whose connector dummy shoves siblings.
    let mut cond_edges: Vec<((usize, usize), (f64, f64, usize))> = cond.into_iter().collect();
    cond_edges.sort_by_key(|&((cs, cd), _)| (cs, cd));
    let mut rank_adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut back_pairs: HashSet<(usize, usize)> = HashSet::new();
    for ((cs, cd), (sum_s, sum_d, count)) in cond_edges {
        if reaches(&rank_adj, cd, cs) {
            back_pairs.insert((cs, cd));
            continue;
        }
        rank_adj.entry(cs).or_default().push(cd);
        let edge = Edge {
            source_perp_offset: Some(sum_s / count as f64),
            target_perp_offset: Some(sum_d / count as f64),
            ..Edge::default()
        };
        vg.add_edge(edge, comp_handles[cs], comp_handles[cd]);
    }
    vg.layout();

    // 6. Expand: member top-left = component top-left + member offset.
    let (deflate_x, deflate_y) = if is_lr {
        (pad_rank / 2.0, pad_perp / 2.0)
    } else {
        (pad_perp / 2.0, pad_rank / 2.0)
    };
    let comp_topleft: Vec<Point> = comp_handles
        .iter()
        .map(|h| {
            let tl = vg.pos(*h).bbox(false).0;
            Point::new(tl.x + deflate_x, tl.y + deflate_y)
        })
        .collect();
    let mut rel: HashMap<usize, Point> = HashMap::new();
    for l in 0..m {
        let c = comp_topleft[comp_of[l]];
        let off = member_offset[l];
        rel.insert(members[l], Point::new(c.x + off.x, c.y + off.y));
    }
    // Normalize the level so it starts at (0, 0).
    let min_x = rel.values().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = rel.values().map(|p| p.y).fold(f64::INFINITY, f64::min);
    for p in rel.values_mut() {
        p.x -= min_x;
        p.y -= min_y;
    }
    let mut bbox = Point::zero();
    for (&g, p) in &rel {
        bbox.x = bbox.x.max(p.x + eff_geom[g].x);
        bbox.y = bbox.y.max(p.y + eff_geom[g].y);
    }

    let back: Vec<(usize, bool)> = ledges
        .iter()
        .map(|&(ti, s, d, horizontal)| {
            (ti, !horizontal && back_pairs.contains(&(comp_of[s], comp_of[d])))
        })
        .collect();

    FlatLayout { rel, bbox, back }
}

/// Stack a composite's per-region layouts into one interior block.
/// Returns `(interior_w, interior_h, region_origins, divider_segments)`.
/// `region_origins[i]` is where region `i`'s content starts within the
/// interior block; the divider segments are in that same interior-content
/// space. A single-region composite produces no dividers.
///
/// Each region is stacked along one axis (Y for `--`, X for `||`) and
/// positioned on the free axis by this rule: **start-align when the free
/// axis is the diagram's rank axis** (so parallel regions begin together),
/// **center when it is the perpendicular axis** (so the flows line up).
fn stack_regions(
    region_fls: &[FlatLayout],
    orient: RegionOrient,
    is_lr: bool,
) -> (f64, f64, Vec<Point>, Vec<(Point, Point)>) {
    if region_fls.len() <= 1 {
        let bb = region_fls.first().map(|fl| fl.bbox).unwrap_or(Point::zero());
        return (bb.x, bb.y, vec![Point::zero()], Vec::new());
    }
    let mut origins = Vec::with_capacity(region_fls.len());
    let mut segs = Vec::new();
    match orient {
        RegionOrient::Horizontal => {
            // `--`: regions stacked top-to-bottom; free axis is x. x is the
            // rank axis only in an LR diagram → start-align there, center
            // in TB so the vertical chains line up.
            let max_w = region_fls.iter().map(|fl| fl.bbox.x).fold(0.0_f64, f64::max);
            let mut cursor = 0.0_f64;
            for (ri, fl) in region_fls.iter().enumerate() {
                let x = if is_lr { 0.0 } else { (max_w - fl.bbox.x) / 2.0 };
                origins.push(Point::new(x, cursor));
                cursor += fl.bbox.y;
                if ri + 1 < region_fls.len() {
                    let dy = cursor + REGION_GAP_PT / 2.0;
                    segs.push((Point::new(0.0, dy), Point::new(max_w, dy)));
                    cursor += REGION_GAP_PT;
                }
            }
            (max_w, cursor, origins, segs)
        }
        RegionOrient::Vertical => {
            // `||`: regions side by side; free axis is y. y is the rank
            // axis only in a TB diagram → start-align there so parallel
            // regions begin at the same height, center in LR.
            let max_h = region_fls.iter().map(|fl| fl.bbox.y).fold(0.0_f64, f64::max);
            let mut cursor = 0.0_f64;
            for (ri, fl) in region_fls.iter().enumerate() {
                let y = if is_lr { (max_h - fl.bbox.y) / 2.0 } else { 0.0 };
                origins.push(Point::new(cursor, y));
                cursor += fl.bbox.x;
                if ri + 1 < region_fls.len() {
                    let dx = cursor + REGION_GAP_PT / 2.0;
                    segs.push((Point::new(dx, 0.0), Point::new(dx, max_h)));
                    cursor += REGION_GAP_PT;
                }
            }
            (cursor, max_h, origins, segs)
        }
    }
}

/// Compute the absolute top-left + effective size of every node.
///
/// Composite states are laid out recursively: each composite's interior
/// is a sub-diagram with its own `layout_flat` pass, the resulting bbox
/// fixes the composite's frame size, and that size feeds the parent
/// level's layout. Positions are propagated top-down at the end.
fn layout_nodes(diag: &StateDiagram, base_geoms: &[NodeGeom], orientation: Orientation) -> Layout {
    let n = diag.nodes.len();
    let is_lr = matches!(orientation, Orientation::LeftToRight);
    let idx: HashMap<&str, usize> = diag
        .nodes
        .iter()
        .enumerate()
        .map(|(i, nd)| (nd.id.as_str(), i))
        .collect();

    let parent_of: Vec<Option<usize>> = diag
        .nodes
        .iter()
        .map(|nd| nd.parent.as_deref().and_then(|p| idx.get(p).copied()))
        .collect();
    let children_of: Vec<Vec<usize>> = diag
        .nodes
        .iter()
        .map(|nd| {
            nd.children
                .iter()
                .filter_map(|c| idx.get(c.as_str()).copied())
                .collect()
        })
        .collect();
    // A node's parent is always created before it, so a forward pass over
    // indices yields correct depths.
    let mut depth = vec![0usize; n];
    for i in 0..n {
        if let Some(p) = parent_of[i] {
            depth[i] = depth[p] + 1;
        }
    }

    // Resolved transition endpoints (drop self-loops + dangling refs).
    let all_edges: Vec<(usize, usize, usize, bool)> = diag
        .transitions
        .iter()
        .enumerate()
        .filter_map(|(ti, tr)| {
            let s = *idx.get(tr.from.as_str())?;
            let d = *idx.get(tr.to.as_str())?;
            if s == d {
                None
            } else {
                Some((ti, s, d, tr.horizontal))
            }
        })
        .collect();

    let mut eff_geom: Vec<Point> = base_geoms.iter().map(|g| g.size).collect();
    let mut frame_offset: Vec<Point> = vec![Point::zero(); n];
    let mut interior: Vec<HashMap<usize, Point>> = vec![HashMap::new(); n];
    // Frame-relative divider segments per composite (empty unless it has
    // `--` / `||` concurrent regions).
    let mut dividers: Vec<Vec<(Point, Point)>> = vec![Vec::new(); n];
    let mut back = vec![false; diag.transitions.len()];

    // composite index → its RegionGroup (only composites with a divider).
    let region_of: HashMap<usize, &RegionGroup> = diag
        .regions
        .iter()
        .filter_map(|rg| idx.get(rg.composite_id.as_str()).map(|&ci| (ci, rg)))
        .collect();

    // Lay out composites deepest-first so a composite's interior bbox is
    // known before its parent level needs its frame size.
    let mut composites: Vec<usize> = (0..n)
        .filter(|&i| diag.nodes[i].kind == StateKind::Composite)
        .collect();
    composites.sort_by_key(|&c| std::cmp::Reverse(depth[c]));
    for &c in &composites {
        // Region partitions: one per `--` / `||` section, or a single
        // partition holding every child for a plain composite.
        let parts: Vec<Vec<usize>> = match region_of.get(&c) {
            Some(rg) => rg
                .partitions
                .iter()
                .map(|p| {
                    p.iter()
                        .filter_map(|id| idx.get(id.as_str()).copied())
                        .collect()
                })
                .collect(),
            None => vec![children_of[c].clone()],
        };
        let orient = region_of
            .get(&c)
            .map(|rg| rg.orientation)
            .unwrap_or(RegionOrient::Horizontal);

        // Lay out each region independently via the flat placer. A
        // cross-region transition simply drops out (its endpoints don't
        // lift into any single region's member set).
        let region_fls: Vec<FlatLayout> = parts
            .iter()
            .map(|part| {
                let edges = lift_edges(&all_edges, part, &parent_of);
                let fl = layout_flat(part, &eff_geom, &edges, orientation);
                for &(ti, b) in &fl.back {
                    back[ti] |= b;
                }
                fl
            })
            .collect();

        // Stack the regions and place each region's nodes into the
        // composite's interior map.
        let (mut interior_w, mut interior_h, region_origin, seg) =
            stack_regions(&region_fls, orient, is_lr);
        // Reserve perpendicular-axis room for any interior back-edge's C-bow
        // plus its label so the painter draws it inside the composite frame.
        // The reserve is symmetric (added on both perpendicular sides) so
        // the interior content stays centered; `bow_reserve` is the per-side
        // amount, sized to the widest interior back-edge label.
        let mut bow_reserve = 0.0_f64;
        for fl in &region_fls {
            for &(ti, is_back) in &fl.back {
                if !is_back {
                    continue;
                }
                let need = match back_edge_label_width(&diag.transitions[ti]) {
                    // bow apex (`bow + 3pt`) + label width + margin.
                    Some(w) => 33.0 + w + 6.0,
                    // just the C-bow line (curve apex ~22.5pt past the node).
                    None => 25.0,
                };
                bow_reserve = bow_reserve.max(need);
            }
        }
        let bow_shift = if is_lr {
            Point::new(0.0, bow_reserve)
        } else {
            Point::new(bow_reserve, 0.0)
        };
        if is_lr {
            interior_h += 2.0 * bow_reserve;
        } else {
            interior_w += 2.0 * bow_reserve;
        }
        let seg: Vec<(Point, Point)> = seg
            .into_iter()
            .map(|(a, b)| (a.add(bow_shift), b.add(bow_shift)))
            .collect();
        for (ri, fl) in region_fls.iter().enumerate() {
            let o = region_origin[ri].add(bow_shift);
            for (&g, p) in &fl.rel {
                interior[c].insert(g, Point::new(o.x + p.x, o.y + p.y));
            }
        }

        let label_w = diag.nodes[c].display.chars().count() as f64 * CHAR_W_PT + 16.0;
        let outer_w = (interior_w + 2.0 * COMPOSITE_PAD_PT).max(label_w + 2.0 * COMPOSITE_PAD_PT);
        let outer_h = interior_h + 2.0 * COMPOSITE_PAD_PT + COMPOSITE_LABEL_BAND_PT;
        eff_geom[c] = Point::new(outer_w, outer_h);
        // Interior content is centered horizontally; the label band sits
        // above it.
        let foff = Point::new(
            (outer_w - interior_w) / 2.0,
            COMPOSITE_PAD_PT + COMPOSITE_LABEL_BAND_PT,
        );
        frame_offset[c] = foff;
        // Stretch each divider across the full frame interior (the stacker
        // sized them only to the content block) and store frame-relative.
        dividers[c] = seg
            .into_iter()
            .map(|(a, _b)| match orient {
                RegionOrient::Horizontal => {
                    let y = foff.y + a.y;
                    (
                        Point::new(COMPOSITE_PAD_PT, y),
                        Point::new(outer_w - COMPOSITE_PAD_PT, y),
                    )
                }
                RegionOrient::Vertical => {
                    let x = foff.x + a.x;
                    (
                        Point::new(x, COMPOSITE_LABEL_BAND_PT),
                        Point::new(x, outer_h - COMPOSITE_PAD_PT),
                    )
                }
            })
            .collect();
    }

    // Top level.
    let top_members: Vec<usize> = (0..n).filter(|&i| parent_of[i].is_none()).collect();
    let top_edges = lift_edges(&all_edges, &top_members, &parent_of);
    let top_fl = layout_flat(&top_members, &eff_geom, &top_edges, orientation);
    for &(ti, b) in &top_fl.back {
        back[ti] |= b;
    }

    // Propagate absolute positions: top members first, then each composite
    // shallowest-first (parent placed before its children).
    let mut top_lefts = vec![Point::zero(); n];
    for &mem in &top_members {
        if let Some(p) = top_fl.rel.get(&mem) {
            top_lefts[mem] = *p;
        }
    }
    let mut composites_pre = composites.clone();
    composites_pre.sort_by_key(|&c| depth[c]);
    for &c in &composites_pre {
        let base = top_lefts[c].add(frame_offset[c]);
        for (&child, &child_rel) in &interior[c] {
            top_lefts[child] = base.add(child_rel);
        }
    }

    Layout {
        top_lefts,
        eff_geom,
        back,
        dividers,
    }
}

pub fn emit(out: &mut String, diag: &StateDiagram) {
    let _ = &diag.skinparams; // S4: skinparam state* → preamble

    if diag.nodes.is_empty() {
        out.push_str("#state-layout()\n");
        return;
    }

    let geoms: Vec<NodeGeom> = diag.nodes.iter().map(node_geom).collect();

    let orientation = match diag.direction {
        LayoutDirection::TopToBottom => Orientation::TopToBottom,
        LayoutDirection::LeftToRight => Orientation::LeftToRight,
    };

    let id_to_idx = |id: &str| diag.nodes.iter().position(|n| n.id == id);

    let layout = layout_nodes(diag, &geoms, orientation);
    let mut top_lefts = layout.top_lefts;
    let eff_geom = layout.eff_geom;
    let back = layout.back;
    let dividers = layout.dividers;
    let is_lr = matches!(orientation, Orientation::LeftToRight);

    // Normalize so the content starts at (MARGIN, MARGIN).
    let min_x = top_lefts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = top_lefts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    for p in &mut top_lefts {
        p.x = p.x - min_x + MARGIN_PT;
        p.y = p.y - min_y + MARGIN_PT;
    }

    // Per-back-edge bow side. PlantUML routes a back-edge around the
    // *outside* of the graph; bowing toward whichever perpendicular
    // extreme the edge's endpoints sit nearer to keeps it from crossing
    // the interior. `"min"` = the low side of the perpendicular axis
    // (left in TB, top in LR); `"max"` = the high side.
    let perp_center = |i: usize| -> f64 {
        if is_lr {
            top_lefts[i].y + eff_geom[i].y / 2.0
        } else {
            top_lefts[i].x + eff_geom[i].x / 2.0
        }
    };
    let perp_lo = (0..diag.nodes.len())
        .map(|i| if is_lr { top_lefts[i].y } else { top_lefts[i].x })
        .fold(f64::INFINITY, f64::min);
    let perp_hi = (0..diag.nodes.len())
        .map(|i| {
            if is_lr {
                top_lefts[i].y + eff_geom[i].y
            } else {
                top_lefts[i].x + eff_geom[i].x
            }
        })
        .fold(0.0_f64, f64::max);
    let bow_side: Vec<&'static str> = diag
        .transitions
        .iter()
        .enumerate()
        .map(|(ti, tr)| {
            if !back[ti] {
                return "max";
            }
            let (Some(s), Some(d)) = (id_to_idx(&tr.from), id_to_idx(&tr.to)) else {
                return "max";
            };
            // An interior back-edge always bows toward the high side —
            // that's the side `layout_nodes` reserved room for inside the
            // enclosing composite frame.
            if diag.nodes[s].parent.is_some() {
                return "max";
            }
            let pos = (perp_center(s) + perp_center(d)) / 2.0;
            // Strict `<` so an exactly-centered edge (e.g. a single-column
            // diagram) ties to the high side, matching PlantUML's default.
            if pos - perp_lo < perp_hi - pos {
                "min"
            } else {
                "max"
            }
        })
        .collect();

    // Reserve space for self-loop arcs and back-edge bows — drawn by the
    // painter outside the node bboxes, on the perpendicular axis. Self
    // loops always bow toward the high side; back-edges bow per `bow_side`.
    // Only *top-level* back-edges need a canvas reserve — an interior
    // back-edge's bow is already contained by its composite frame (see
    // `COMPOSITE_BACK_BOW_PT` in `layout_nodes`).
    let has_self_loop = diag
        .transitions
        .iter()
        .any(|tr| id_to_idx(&tr.from).is_some() && tr.from == tr.to);
    let is_top_level = |ti: usize| -> bool {
        diag.transitions
            .get(ti)
            .and_then(|tr| id_to_idx(&tr.from))
            .map(|s| diag.nodes[s].parent.is_none())
            .unwrap_or(true)
    };
    let back_lo = back
        .iter()
        .enumerate()
        .zip(&bow_side)
        .any(|((ti, &b), &side)| b && side == "min" && is_top_level(ti));
    let back_hi = back
        .iter()
        .enumerate()
        .zip(&bow_side)
        .any(|((ti, &b), &side)| b && side == "max" && is_top_level(ti));
    let reserve_lo = if back_lo { BACK_BOW_RESERVE_PT } else { 0.0 };
    let reserve_hi = {
        let mut r: f64 = 0.0;
        if has_self_loop {
            r = r.max(SELF_LOOP_RESERVE_PT);
        }
        if back_hi {
            r = r.max(BACK_BOW_RESERVE_PT);
        }
        r
    };

    // Shift the content over by the low-side reserve so a `"min"` bow has
    // room to curl outside the canvas's content area.
    for p in &mut top_lefts {
        if is_lr {
            p.y += reserve_lo;
        } else {
            p.x += reserve_lo;
        }
    }

    // Place notes. A `note … of` sticky sits left / right of its anchor
    // state; a `note on link` sticky sits next to the transition midpoint.
    // The natural position is then pushed further out until it clears
    // every obstacle (see `clear_note_x`).
    struct NoteBox {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        body: String,
        side: &'static str,
        /// Anchor rectangle the dashed connector points at — the anchored
        /// state's bbox for `note … of`, or a degenerate point at the link
        /// midpoint for `note on link`.
        ax: f64,
        ay: f64,
        aw: f64,
        ah: f64,
    }
    let center = |i: usize| -> Point {
        Point::new(
            top_lefts[i].x + eff_geom[i].x / 2.0,
            top_lefts[i].y + eff_geom[i].y / 2.0,
        )
    };

    // Obstacles a note must clear: every node bbox plus — in TB, where
    // notes and bows share the x axis — the bands occupied by back-edge
    // bows and self-loop arcs. (In LR those bows curl on the y axis, clear
    // of an x-moving note, so only node bboxes matter.)
    let mut obstacles: Vec<(f64, f64, f64, f64)> = (0..diag.nodes.len())
        .map(|i| (top_lefts[i].x, top_lefts[i].y, eff_geom[i].x, eff_geom[i].y))
        .collect();
    if !is_lr {
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (id_to_idx(&tr.from), id_to_idx(&tr.to)) else {
                continue;
            };
            let (sp, sg, dp, dg) = (top_lefts[s], eff_geom[s], top_lefts[d], eff_geom[d]);
            if tr.from == tr.to {
                // Self-loop arc bulges right of the node.
                obstacles.push((sp.x + sg.x, sp.y, 32.0, sg.y));
            } else if back[ti] {
                let y0 = sp.y.min(dp.y);
                let y1 = (sp.y + sg.y).max(dp.y + dg.y);
                if bow_side[ti] == "min" {
                    let x = sp.x.min(dp.x);
                    obstacles.push((x - 36.0, y0, 36.0, y1 - y0));
                } else {
                    let x = (sp.x + sg.x).max(dp.x + dg.x);
                    obstacles.push((x, y0, 36.0, y1 - y0));
                }
            }
        }
    }
    // Slide a note box along x (away from its anchor) until it overlaps no
    // obstacle. `side` is the direction it may travel.
    let clear_note_x = |mut nx: f64, ny: f64, w: f64, h: f64, side: &str| -> f64 {
        let (y0, y1) = (ny, ny + h);
        for _ in 0..=obstacles.len() {
            let mut moved = false;
            for &(ox, oy, ow, oh) in &obstacles {
                if oy + oh <= y0 || oy >= y1 {
                    continue; // no vertical overlap
                }
                if ox < nx + w && ox + ow > nx {
                    if side == "right" {
                        nx = ox + ow + NOTE_GAP_PT;
                    } else {
                        nx = ox - NOTE_GAP_PT - w;
                    }
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        nx
    };

    let mut note_boxes: Vec<NoteBox> = Vec::new();
    for note in &diag.notes {
        match &note.anchor {
            NoteAnchor::OfNode { node_id, side } => {
                let Some(ai) = id_to_idx(node_id) else { continue };
                let sz = note_geom(&note.body);
                let ap = top_lefts[ai];
                let ag = eff_geom[ai];
                let cy = ap.y + ag.y / 2.0 - sz.y / 2.0;
                let (natural_nx, side_kw) = match side {
                    NotePosition::RightOf => (ap.x + ag.x + NOTE_GAP_PT, "right"),
                    // `left of` and the unused `over` both fall to the left.
                    _ => (ap.x - NOTE_GAP_PT - sz.x, "left"),
                };
                let nx = clear_note_x(natural_nx, cy, sz.x, sz.y, side_kw);
                note_boxes.push(NoteBox {
                    x: nx,
                    y: cy,
                    w: sz.x,
                    h: sz.y,
                    body: note.body.clone(),
                    side: side_kw,
                    ax: ap.x,
                    ay: ap.y,
                    aw: ag.x,
                    ah: ag.y,
                });
            }
            NoteAnchor::OnLink { transition_idx } => {
                let Some(tr) = diag.transitions.get(*transition_idx) else {
                    continue;
                };
                let (Some(si), Some(di)) = (id_to_idx(&tr.from), id_to_idx(&tr.to)) else {
                    continue;
                };
                let (sc, dc) = (center(si), center(di));
                let mx = (sc.x + dc.x) / 2.0;
                let my = (sc.y + dc.y) / 2.0;
                let sz = note_geom(&note.body);
                let cy = my - sz.y / 2.0;
                // Sticky sits to the right of the link midpoint; its dashed
                // connector exits the left edge back toward the midpoint.
                let nx = clear_note_x(mx + NOTE_GAP_PT, cy, sz.x, sz.y, "right");
                note_boxes.push(NoteBox {
                    x: nx,
                    y: cy,
                    w: sz.x,
                    h: sz.y,
                    body: note.body.clone(),
                    side: "right",
                    ax: mx,
                    ay: my,
                    aw: 0.0,
                    ah: 0.0,
                });
            }
            NoteAnchor::Floating { .. } => continue,
        }
    }

    // A left-of note (or a `"min"` bow on a left-edge node) may have pushed
    // content past x = 0 / y = 0. Re-normalize everything together.
    let content_min_x = top_lefts
        .iter()
        .map(|p| p.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x, n.ax]))
        .fold(f64::INFINITY, f64::min);
    let content_min_y = top_lefts
        .iter()
        .map(|p| p.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y, n.ay]))
        .fold(f64::INFINITY, f64::min);
    let shift_x = (MARGIN_PT - content_min_x).max(0.0);
    let shift_y = (MARGIN_PT - content_min_y).max(0.0);
    for p in &mut top_lefts {
        p.x += shift_x;
        p.y += shift_y;
    }
    for nb in &mut note_boxes {
        nb.x += shift_x;
        nb.y += shift_y;
        nb.ax += shift_x;
        nb.ay += shift_y;
    }

    let max_x = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.x + g.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x + n.w, n.ax + n.aw]))
        .fold(0.0_f64, f64::max);
    let max_y = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.y + g.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y + n.h, n.ay + n.ah]))
        .fold(0.0_f64, f64::max);
    let (page_w, page_h) = if is_lr {
        (max_x + MARGIN_PT, max_y + reserve_hi + MARGIN_PT)
    } else {
        (max_x + reserve_hi + MARGIN_PT, max_y + MARGIN_PT)
    };

    // ----- emit -----
    out.push_str("#state-layout(\n");

    out.push_str("  nodes: (\n");
    for (i, n) in diag.nodes.iter().enumerate() {
        let p = top_lefts[i];
        let g = eff_geom[i];
        out.push_str("    (");
        write!(out, "id: \"{}\", ", typst_str_escape(&n.id)).unwrap();
        write!(out, "kind: \"{}\", ", n.kind.keyword()).unwrap();
        write!(
            out,
            "x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, ",
            p.x, p.y, g.x, g.y
        )
        .unwrap();
        write!(out, "display: \"{}\", ", typst_str_escape(&n.display)).unwrap();
        out.push_str("body: (");
        for (bi, row) in n.body.iter().enumerate() {
            if bi > 0 {
                out.push_str(", ");
            }
            write!(out, "\"{}\"", typst_str_escape(row)).unwrap();
        }
        if n.body.len() == 1 {
            out.push(',');
        }
        out.push_str("), ");
        match n.fill.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "fill: {c}, ").unwrap(),
            None => out.push_str("fill: none, "),
        }
        write!(out, "border-style: \"{}\", ", border_style_kw(n.border_style)).unwrap();
        match n.border_color.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "border-color: {c}").unwrap(),
            None => out.push_str("border-color: none"),
        }
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    out.push_str("  transitions: (\n");
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if id_to_idx(&tr.from).is_none() || id_to_idx(&tr.to).is_none() {
            continue;
        }
        out.push_str("    (");
        write!(out, "from: \"{}\", ", typst_str_escape(&tr.from)).unwrap();
        write!(out, "to: \"{}\", ", typst_str_escape(&tr.to)).unwrap();
        emit_opt_str(out, "event", tr.event.as_deref());
        emit_opt_str(out, "guard", tr.guard.as_deref());
        emit_opt_str(out, "action", tr.action.as_deref());
        write!(out, "style: \"{}\", ", line_style_kw(tr.line_style)).unwrap();
        match tr.color.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "color: {c}, ").unwrap(),
            None => out.push_str("color: none, "),
        }
        let _ = direction_kw(tr.direction); // S2+: direction-biased routing
        write!(out, "self-loop: {}, ", tr.from == tr.to).unwrap();
        write!(out, "back: {}, ", back[ti]).unwrap();
        write!(out, "bow-side: \"{}\"", bow_side[ti]).unwrap();
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    out.push_str("  notes: (\n");
    for nb in &note_boxes {
        out.push_str("    (");
        write!(
            out,
            "x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, ",
            nb.x, nb.y, nb.w, nb.h
        )
        .unwrap();
        write!(out, "body: \"{}\", ", typst_str_escape(&nb.body)).unwrap();
        write!(out, "side: \"{}\", ", nb.side).unwrap();
        write!(
            out,
            "anchor: (x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt)",
            nb.ax, nb.ay, nb.aw, nb.ah
        )
        .unwrap();
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    // Concurrent-region dividers — emitted only when a composite actually
    // has `--` / `||` regions, so plain diagrams keep their `regions: ()`
    // default and their golden output unchanged.
    if dividers.iter().any(|d| !d.is_empty()) {
        out.push_str("  regions: (\n");
        for (ci, segs) in dividers.iter().enumerate() {
            if segs.is_empty() {
                continue;
            }
            let base = top_lefts[ci];
            let orient = diag
                .regions
                .iter()
                .find(|rg| rg.composite_id == diag.nodes[ci].id)
                .map(|rg| match rg.orientation {
                    RegionOrient::Vertical => "vertical",
                    RegionOrient::Horizontal => "horizontal",
                })
                .unwrap_or("horizontal");
            out.push_str("    (");
            write!(out, "parent: \"{}\", ", typst_str_escape(&diag.nodes[ci].id)).unwrap();
            write!(out, "orientation: \"{orient}\", ").unwrap();
            out.push_str("dividers: (");
            for (a, b) in segs {
                write!(
                    out,
                    "(x0: {:.2}pt, y0: {:.2}pt, x1: {:.2}pt, y1: {:.2}pt), ",
                    base.x + a.x,
                    base.y + a.y,
                    base.x + b.x,
                    base.y + b.y,
                )
                .unwrap();
            }
            out.push_str(")),\n");
        }
        out.push_str("  ),\n");
    }

    write!(out, "  page: ({page_w:.2}pt, {page_h:.2}pt),\n").unwrap();
    match &diag.title {
        Some(t) => write!(out, "  title: \"{}\",\n", typst_str_escape(t)).unwrap(),
        None => out.push_str("  title: none,\n"),
    }
    write!(
        out,
        "  hide-empty-description: {},\n",
        diag.hide_empty_description
    )
    .unwrap();
    write!(
        out,
        "  direction: \"{}\",\n",
        if is_lr { "lr" } else { "tb" }
    )
    .unwrap();
    out.push_str(")\n");
}

fn emit_opt_str(out: &mut String, key: &str, val: Option<&str>) {
    match val {
        Some(v) => write!(out, "{key}: \"{}\", ", typst_str_escape(v)).unwrap(),
        None => write!(out, "{key}: none, ").unwrap(),
    }
}

fn line_style_kw(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
    }
}

fn border_style_kw(s: Option<BorderStyle>) -> &'static str {
    match s {
        Some(BorderStyle::Solid) | None => "solid",
        Some(BorderStyle::Dashed) => "dashed",
        Some(BorderStyle::Dotted) => "dotted",
        Some(BorderStyle::Bold) => "bold",
    }
}

fn direction_kw(d: Option<Direction>) -> &'static str {
    match d {
        Some(Direction::Up) => "up",
        Some(Direction::Down) => "down",
        Some(Direction::Left) => "left",
        Some(Direction::Right) => "right",
        None => "none",
    }
}

fn typst_str_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Translate a PlantUML color spec (`#LightBlue`, `#ABC`, `red`) to a Typst
/// `rgb(...)` literal. Returns `None` for an unparseable spec.
fn puml_color_to_typst(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let lower = hex.to_ascii_lowercase();
    let named = match lower.as_str() {
        "red" => Some("FF0000"),
        "blue" => Some("0000FF"),
        "green" => Some("008000"),
        "yellow" => Some("FFFF00"),
        "orange" => Some("FFA500"),
        "black" => Some("000000"),
        "white" => Some("FFFFFF"),
        "gray" | "grey" => Some("808080"),
        "lightblue" => Some("ADD8E6"),
        "lightgreen" => Some("90EE90"),
        "lightyellow" => Some("FFFFE0"),
        "lightgray" | "lightgrey" => Some("D3D3D3"),
        "pink" => Some("FFC0CB"),
        _ => None,
    };
    let final_hex = match named {
        Some(h) => h.to_string(),
        None => {
            if hex.chars().all(|c| c.is_ascii_hexdigit()) && (hex.len() == 3 || hex.len() == 6) {
                hex.to_string()
            } else {
                return None;
            }
        }
    };
    Some(format!("rgb(\"#{final_hex}\")"))
}
