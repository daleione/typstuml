//! Recursive cluster layout (dot's model).
//!
//! Each composite's interior is laid out as its own sub-graph (`layout_flat`),
//! the resulting bbox fixes the composite's frame size, and that frame becomes
//! a single box node in the parent level. Concurrent regions are sibling
//! sub-layouts inside their composite, stacked by `stack_regions`.

use std::collections::{HashMap, HashSet};

use crate::ir::{RegionGroup, RegionOrient, StateDiagram, StateKind};
use crate::layout::dag::NodeHandle;
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};

use super::geom::NodeGeom;

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
/// Gap between adjacent concurrent regions inside a composite state — the
/// `--` / `||` divider line is centered in this band.
const REGION_GAP_PT: f64 = 26.0;

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
pub(super) struct Layout {
    /// Absolute top-left of every node, indexed like `diag.nodes`.
    pub(super) top_lefts: Vec<Point>,
    /// Effective size of every node, indexed like `diag.nodes`. Equals the
    /// heuristic `node_geom` for simple states / pseudostates; for a
    /// composite state it is the computed frame size (interior bbox +
    /// padding + label band).
    pub(super) eff_geom: Vec<Point>,
    /// Per-transition flag (indexed like `diag.transitions`): `true` when
    /// the transition was identified as a back-edge (it would have formed
    /// a cycle in the rank graph). The painter draws these as a side-bow
    /// instead of a straight line so they don't shoot back through the
    /// intervening states.
    pub(super) back: Vec<bool>,
    /// Concurrent-region divider segments, indexed like `diag.nodes` and
    /// stored *relative to the composite frame's top-left*. Non-empty only
    /// for composite states with a `--` / `||` divider.
    pub(super) dividers: Vec<Vec<(Point, Point)>>,
    /// Per-transition routed polyline in absolute coords (same space as
    /// `top_lefts`): `[start, ..mid-points, end]`. Currently always empty —
    /// the layout places composites as wide boxes, so `route_transitions`
    /// detours obstructed edges around them from final node positions and
    /// the painter draws unobstructed edges straight. Retained for a future
    /// connector-chain router.
    pub(super) waypoints: HashMap<usize, Vec<Point>>,
    /// Per-transition reserved label position (absolute centre), for
    /// transitions whose label was laid out as a dot-style label node.
    /// The painter draws the label here instead of computing a midpoint.
    pub(super) label_pos: HashMap<usize, Point>,
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
    /// Transition index → reserved label centre, in the same normalized
    /// frame as `rel`. Present only for labelled forward edges that were
    /// laid out as a dot-style label node at this level.
    label_pos: HashMap<usize, Point>,
    /// Transition index → interior connector-chain centres (the side lane a
    /// long forward edge runs through), in the same normalized frame as
    /// `rel`. Empty for adjacent-rank edges.
    waypoints: HashMap<usize, Vec<Point>>,
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
    diag: &StateDiagram,
    members: &[usize],
    eff_geom: &[Point],
    edges: &[(usize, usize, usize, bool)],
    label_sizes: &[Option<(f64, f64)>],
    orientation: Orientation,
) -> FlatLayout {
    let m = members.len();
    if m == 0 {
        return FlatLayout {
            rel: HashMap::new(),
            bbox: Point::zero(),
            back: edges.iter().map(|&(ti, ..)| (ti, false)).collect(),
            label_pos: HashMap::new(),
            waypoints: HashMap::new(),
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

    // 5. Condensed graph — one box per component. Edges are dot's network
    // simplex on both axes (rank = minlen-honouring NS, x = NS on the
    // auxiliary graph), with each labelled forward edge carrying a
    // **label node** (dot's edge-label virtual node) that reserves a rank
    // + perpendicular slot so the diagram spreads to fit its label.
    let mut vg = VisualGraph::new(orientation);
    vg.enable_ns_rank(); // dot's minlen-honouring rank assignment
    vg.enable_ns_xcoord(); // dot's network-simplex x-assignment
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
    let is_comp_l = |l: usize| diag.nodes[members[l]].kind == StateKind::Composite;
    // Feedback-arc-set pass: an edge whose target already reaches its
    // source is a cycle (back-edge) — keep it out of the rank graph so the
    // placer doesn't reverse it into a rank-skipping long edge; the painter
    // bows it instead. Forward edges (including rank-skipping ones from
    // `--->` minlen) stay so the source fans out toward its edge lane.
    let mut rank_adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut back_pairs: HashSet<(usize, usize)> = HashSet::new();
    let mut label_handle: HashMap<usize, NodeHandle> = HashMap::new();
    // Forward (non-back) transition → its component endpoints, for
    // reconstructing the connector chain into routing waypoints.
    let mut edge_comp: HashMap<usize, (usize, usize)> = HashMap::new();
    for &(ti, s, d, horizontal) in &ledges {
        if horizontal {
            continue;
        }
        let (cs, cd) = (comp_of[s], comp_of[d]);
        if cs == cd {
            continue;
        }
        if reaches(&rank_adj, cd, cs) {
            back_pairs.insert((cs, cd));
            continue;
        }
        rank_adj.entry(cs).or_default().push(cd);
        edge_comp.insert(ti, (cs, cd));
        // dot's minlen from the dash count. A label node sits one rank below
        // the source; the target keeps the full minlen below the label so
        // `-->` and `--->` still differ by a rank. Only give a label node to
        // edges between two *leaf* members — an edge touching a composite box
        // keeps the painter's own label placement (the box already reserves
        // ample room and a label node there fights the frame).
        let minlen = diag.transitions[ti].min_rank.max(1);
        let both_leaf = !is_comp_l(s) && !is_comp_l(d);
        let mk = |ml: usize| Edge {
            min_rank: ml,
            ..Edge::default()
        };
        match (label_sizes.get(ti).copied().flatten(), both_leaf) {
            (Some((lw, lh_h)), true) => {
                let lhn = vg.add_node(Element::new_box(Point::new(lw, lh_h), orientation));
                vg.add_edge(mk(1), comp_handles[cs], lhn);
                vg.add_edge(mk(minlen), lhn, comp_handles[cd]);
                label_handle.insert(ti, lhn);
            }
            _ => {
                vg.add_edge(mk(minlen), comp_handles[cs], comp_handles[cd]);
            }
        }
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

    // History pseudostates re-enter the composite "from after": the resume
    // transition comes from a state reached by leaving the composite, so in
    // dot it is a reversed back-edge and network simplex ranks the history
    // node at the *bottom* of the cluster (minimising that edge). Our
    // per-level placer can't see the external resume edge, so an isolated
    // history node (no internal edges) defaults to rank 0 — beside the entry
    // — and the resume arc then bows across the interior. Re-seat it on the
    // interior's last rank, off to the perpendicular side, so the resume
    // edge enters cleanly from the exit side.
    {
        let is_hist = |l: usize| {
            matches!(
                diag.nodes[members[l]].kind,
                StateKind::History | StateKind::DeepHistory
            )
        };
        let mut incident = vec![false; m];
        for &(_, s, d, _) in &ledges {
            incident[s] = true;
            incident[d] = true;
        }
        for l in 0..m {
            if !is_hist(l) || incident[l] {
                continue;
            }
            let g = members[l];
            let (mut rank_hi, mut perp_hi) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            let mut found = false;
            for o in 0..m {
                if o == l || is_hist(o) {
                    continue;
                }
                let p = rel[&members[o]];
                let gg = eff_geom[members[o]];
                found = true;
                if is_lr {
                    rank_hi = rank_hi.max(p.x + gg.x);
                    perp_hi = perp_hi.max(p.y + gg.y);
                } else {
                    rank_hi = rank_hi.max(p.y + gg.y);
                    perp_hi = perp_hi.max(p.x + gg.x);
                }
            }
            if !found {
                continue;
            }
            let gg = eff_geom[g];
            // Bottom-aligned on the rank axis (the exit rank), placed past
            // the interior on the perpendicular side with a gap.
            let new = if is_lr {
                Point::new(rank_hi - gg.x, perp_hi + COMP_GAP_PT)
            } else {
                Point::new(perp_hi + COMP_GAP_PT, rank_hi - gg.y)
            };
            rel.insert(g, new);
        }
    }

    // Label-node centres + extents, in the same VG frame as `rel`.
    let mut label_pos: HashMap<usize, Point> = HashMap::new();
    let mut label_box: Vec<(Point, Point)> = Vec::new();
    for (&ti, &lhn) in &label_handle {
        let (lo, hi) = vg.pos(lhn).bbox(false);
        label_pos.insert(ti, Point::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0));
        label_box.push((lo, hi));
    }

    // Connector-chain waypoints (dot's "edge follows its virtual-node
    // chain"): each lowered edge is a handle chain `[src, ..connectors.., dst]`.
    // The connector centres are the side-lane the long bypass edge runs
    // through — keeping them as waypoints spreads parallel skip-edges into
    // distinct lanes and lets the painter bow them into splines, instead of
    // a straight diagonal squeezed against the spine.
    let center = |h: NodeHandle| -> Point {
        let (lo, hi) = vg.pos(h).bbox(false);
        Point::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0)
    };
    let mut chain_of: HashMap<(usize, usize), Vec<Point>> = HashMap::new();
    for (_, chain) in vg.iter_edges() {
        if chain.len() < 2 {
            continue;
        }
        let pts: Vec<Point> = chain.iter().map(|h| center(*h)).collect();
        chain_of.insert(
            (
                chain.first().unwrap().get_index(),
                chain.last().unwrap().get_index(),
            ),
            pts,
        );
    }
    // Interior connector centres for an a→b handle segment (drops endpoints).
    let seg_mids = |a: NodeHandle, b: NodeHandle| -> Vec<Point> {
        let inner = |pts: &Vec<Point>| -> Vec<Point> {
            if pts.len() > 2 {
                pts[1..pts.len() - 1].to_vec()
            } else {
                Vec::new()
            }
        };
        if let Some(pts) = chain_of.get(&(a.get_index(), b.get_index())) {
            inner(pts)
        } else if let Some(pts) = chain_of.get(&(b.get_index(), a.get_index())) {
            let mut m = inner(pts);
            m.reverse();
            m
        } else {
            Vec::new()
        }
    };
    let mut waypoints: HashMap<usize, Vec<Point>> = HashMap::new();
    for (&ti, &(cs, cd)) in &edge_comp {
        let (sh, th) = (comp_handles[cs], comp_handles[cd]);
        let mids = if let Some(&lh) = label_handle.get(&ti) {
            let mut m = seg_mids(sh, lh);
            m.push(center(lh));
            m.extend(seg_mids(lh, th));
            m
        } else {
            seg_mids(sh, th)
        };
        if !mids.is_empty() {
            waypoints.insert(ti, mids);
        }
    }

    // Normalize the level so it starts at (0, 0).
    let min_x = rel.values().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = rel.values().map(|p| p.y).fold(f64::INFINITY, f64::min);
    for p in rel.values_mut() {
        p.x -= min_x;
        p.y -= min_y;
    }
    for p in label_pos.values_mut() {
        p.x -= min_x;
        p.y -= min_y;
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            p.x -= min_x;
            p.y -= min_y;
        }
    }
    let mut bbox = Point::zero();
    for (&g, p) in &rel {
        bbox.x = bbox.x.max(p.x + eff_geom[g].x);
        bbox.y = bbox.y.max(p.y + eff_geom[g].y);
    }
    // A wide label can stick out past every member — keep it inside the bbox
    // (and thus inside an enclosing composite frame).
    for (lo, hi) in &label_box {
        bbox.x = bbox.x.max(hi.x - min_x);
        bbox.y = bbox.y.max(hi.y - min_y);
        let _ = lo;
    }

    let back: Vec<(usize, bool)> = ledges
        .iter()
        .map(|&(ti, s, d, horizontal)| {
            (
                ti,
                !horizontal && back_pairs.contains(&(comp_of[s], comp_of[d])),
            )
        })
        .collect();

    FlatLayout {
        rel,
        bbox,
        back,
        label_pos,
        waypoints,
    }
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
        let bb = region_fls
            .first()
            .map(|fl| fl.bbox)
            .unwrap_or(Point::zero());
        return (bb.x, bb.y, vec![Point::zero()], Vec::new());
    }
    let mut origins = Vec::with_capacity(region_fls.len());
    let mut segs = Vec::new();
    match orient {
        RegionOrient::Horizontal => {
            // `--`: regions stacked top-to-bottom; free axis is x. x is the
            // rank axis only in an LR diagram → start-align there, center
            // in TB so the vertical chains line up.
            let max_w = region_fls
                .iter()
                .map(|fl| fl.bbox.x)
                .fold(0.0_f64, f64::max);
            let mut cursor = 0.0_f64;
            for (ri, fl) in region_fls.iter().enumerate() {
                let x = if is_lr {
                    0.0
                } else {
                    (max_w - fl.bbox.x) / 2.0
                };
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
            let max_h = region_fls
                .iter()
                .map(|fl| fl.bbox.y)
                .fold(0.0_f64, f64::max);
            let mut cursor = 0.0_f64;
            for (ri, fl) in region_fls.iter().enumerate() {
                let y = if is_lr {
                    (max_h - fl.bbox.y) / 2.0
                } else {
                    0.0
                };
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
pub(super) fn layout_nodes(
    diag: &StateDiagram,
    base_geoms: &[NodeGeom],
    label_sizes: &[Option<(f64, f64)>],
    orientation: Orientation,
) -> Layout {
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
    // Frame-relative reserved label centres per composite (label nodes laid
    // out by the per-level placer), translated to absolute on propagate.
    let mut interior_label: Vec<HashMap<usize, Point>> = vec![HashMap::new(); n];
    // Frame-relative connector-chain waypoints per composite, likewise.
    let mut interior_waypoints: Vec<HashMap<usize, Vec<Point>>> = vec![HashMap::new(); n];
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
                let fl = layout_flat(diag, part, &eff_geom, &edges, label_sizes, orientation);
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
                let need = match label_sizes.get(ti).copied().flatten() {
                    // bow apex (`bow + 3pt`) + label width + margin.
                    Some((w, _)) => 33.0 + w + 6.0,
                    // just the C-bow line (curve apex ~22.5pt past the node).
                    None => 25.0,
                };
                bow_reserve = bow_reserve.max(need);
            }
        }
        // Self-loops are dropped from the layout graph (from == to), so they
        // never show up in `fl.back`; reserve for their arc separately. Unlike
        // a back-edge bow (which `bow_reserve` pads symmetrically to keep the
        // interior centered), the painter always bulges a self-loop onto the
        // *high* perpendicular side only — 26pt for the arc plus the label
        // 3pt past it (`states.typ`). So this reserve is one-sided: it widens
        // the frame on the high side while the interior content stays put,
        // matching how dot draws a self-edge in the node's right margin.
        let members: std::collections::HashSet<usize> = parts.iter().flatten().copied().collect();
        let mut loop_reserve = 0.0_f64;
        for (ti, tr) in diag.transitions.iter().enumerate() {
            if tr.from != tr.to {
                continue;
            }
            let Some(&s) = idx.get(tr.from.as_str()) else {
                continue;
            };
            if !members.contains(&s) {
                continue;
            }
            let need = match label_sizes.get(ti).copied().flatten() {
                // arc apex (`26 + 3pt`) + label width + margin.
                Some((w, _)) => 29.0 + w + 6.0,
                // bare arc (bulges ~26pt past the node) + margin.
                None => 32.0,
            };
            loop_reserve = loop_reserve.max(need);
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
            for (&ti, lp) in &fl.label_pos {
                interior_label[c].insert(ti, Point::new(o.x + lp.x, o.y + lp.y));
            }
            for (&ti, chain) in &fl.waypoints {
                interior_waypoints[c].insert(
                    ti,
                    chain
                        .iter()
                        .map(|p| Point::new(o.x + p.x, o.y + p.y))
                        .collect(),
                );
            }
        }

        // Floor the frame width to the label box (measured when available).
        let label_w = base_geoms[c].size.x;
        // Content box (interior + padding, floored to the label). The
        // self-loop reserve is then appended to the high perpendicular side
        // only — never recentered — so the arc has room without pushing the
        // interior off-center or shifting the frame's entry edge.
        let content_w = (interior_w + 2.0 * COMPOSITE_PAD_PT).max(label_w + 2.0 * COMPOSITE_PAD_PT);
        let content_h = interior_h + 2.0 * COMPOSITE_PAD_PT + COMPOSITE_LABEL_BAND_PT;
        let (outer_w, outer_h) = if is_lr {
            (content_w, content_h + loop_reserve)
        } else {
            (content_w + loop_reserve, content_h)
        };
        eff_geom[c] = Point::new(outer_w, outer_h);
        // Interior content is centered within the content box (excluding the
        // one-sided loop reserve); the label band sits above it.
        let foff = Point::new(
            (content_w - interior_w) / 2.0,
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
    let top_fl = layout_flat(
        diag,
        &top_members,
        &eff_geom,
        &top_edges,
        label_sizes,
        orientation,
    );
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
    // Top-level labels are already in the top frame (= absolute, modulo the
    // global normalize emit applies). Composite-interior labels are
    // translated by their frame's absolute origin below.
    let mut label_pos: HashMap<usize, Point> = top_fl.label_pos.clone();
    let mut waypoints: HashMap<usize, Vec<Point>> = top_fl.waypoints.clone();
    let mut composites_pre = composites.clone();
    composites_pre.sort_by_key(|&c| depth[c]);
    for &c in &composites_pre {
        let base = top_lefts[c].add(frame_offset[c]);
        for (&child, &child_rel) in &interior[c] {
            top_lefts[child] = base.add(child_rel);
        }
        for (&ti, lp) in &interior_label[c] {
            label_pos.insert(ti, base.add(*lp));
        }
        for (&ti, chain) in &interior_waypoints[c] {
            waypoints.insert(ti, chain.iter().map(|p| base.add(*p)).collect());
        }
    }

    // Snap entry / exit points so they straddle their composite's border
    // (the glyph's centre sits on the border line) — entry on the
    // rank-start edge (top in TB, left in LR), exit on the rank-end edge —
    // keeping the laid-out perpendicular coordinate.
    for &c in &composites_pre {
        for &child in &children_of[c] {
            let g = eff_geom[child];
            match diag.nodes[child].kind {
                StateKind::EntryPoint => {
                    if is_lr {
                        top_lefts[child].x = top_lefts[c].x - g.x / 2.0;
                    } else {
                        top_lefts[child].y = top_lefts[c].y - g.y / 2.0;
                    }
                }
                StateKind::ExitPoint => {
                    if is_lr {
                        top_lefts[child].x = top_lefts[c].x + eff_geom[c].x - g.x / 2.0;
                    } else {
                        top_lefts[child].y = top_lefts[c].y + eff_geom[c].y - g.y / 2.0;
                    }
                }
                _ => {}
            }
        }
    }

    Layout {
        top_lefts,
        eff_geom,
        back,
        dividers,
        waypoints,
        label_pos,
    }
}
