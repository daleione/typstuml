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
use crate::layout::dag::NodeHandle;
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};
use crate::runtime::MeasurementSet;

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

// ---------------------------------------------------------------------------
// Pass-1 measure probes.
//
// `node_geom` / `note_geom` estimate text width from a char count, which is
// wrong for proportional fonts and CJK. When a `MeasurementSet` is supplied,
// `resolve_node_geom` / `resolve_note_size` use the painter-measured size
// instead (see `state-probe` / `state-note-probe` in `states.typ`).
// ---------------------------------------------------------------------------

/// Stable probe id for a simple / composite state.
fn state_node_id(diagram_idx: usize, node: &StateNode) -> String {
    format!("ms-{diagram_idx}-{}", sanitize_id(&node.id))
}

/// Stable probe id for a note (notes have no user id, so key by index).
fn state_note_id(diagram_idx: usize, note_idx: usize) -> String {
    format!("msn-{diagram_idx}-{note_idx}")
}

/// Collapse an IR node id into a string safe to embed in a probe id.
fn sanitize_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// True iff the diagram has text-bearing content worth measuring — any
/// simple / composite state, or any note. Pseudostates are fixed-size.
pub fn has_probes(diag: &StateDiagram) -> bool {
    diag.nodes
        .iter()
        .any(|n| matches!(n.kind, StateKind::Simple | StateKind::Composite))
        || !diag.notes.is_empty()
}

/// Emit one `#state-probe(...)` per simple / composite state and one
/// `#state-note-probe(...)` per note into the pass-1 source.
pub fn collect_probes(
    diag: &StateDiagram,
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    for node in &diag.nodes {
        if !matches!(node.kind, StateKind::Simple | StateKind::Composite) {
            continue;
        }
        let id = state_node_id(diagram_idx, node);
        write!(
            out,
            "#state-probe(id: \"{}\", display: \"{}\", body: (",
            id,
            typst_str_escape(&node.display)
        )
        .unwrap();
        for (i, row) in node.body.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "\"{}\"", typst_str_escape(row)).unwrap();
        }
        if node.body.len() == 1 {
            out.push(',');
        }
        out.push_str("))\n");
        expected_ids.push(id);
    }
    for (ni, note) in diag.notes.iter().enumerate() {
        let id = state_note_id(diagram_idx, ni);
        writeln!(
            out,
            "#state-note-probe(id: \"{}\", body: \"{}\")",
            id,
            typst_str_escape(&note.body)
        )
        .unwrap();
        expected_ids.push(id);
    }
}

/// Per-node geometry: measured size from pass-1 when available, otherwise
/// the char-count heuristic. Pseudostates are always fixed-size.
fn resolve_node_geom(
    n: &StateNode,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> NodeGeom {
    if matches!(n.kind, StateKind::Simple | StateKind::Composite) {
        if let Some(set) = measurements {
            if let Some(m) = set.get(&state_node_id(diagram_idx, n)) {
                return NodeGeom {
                    size: Point::new(m.width_pt, m.height_pt),
                };
            }
        }
    }
    node_geom(n)
}

/// Note sticky size: measured from pass-1 when available, else heuristic.
fn resolve_note_size(
    note_idx: usize,
    body: &str,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> Point {
    if let Some(set) = measurements {
        if let Some(m) = set.get(&state_note_id(diagram_idx, note_idx)) {
            return Point::new(m.width_pt, m.height_pt);
        }
    }
    note_geom(body)
}

/// Heuristic bounding box for one node.
fn node_geom(n: &StateNode) -> NodeGeom {
    let size = match n.kind {
        StateKind::Initial | StateKind::Final => Point::new(18.0, 18.0),
        StateKind::EntryPoint | StateKind::ExitPoint => Point::new(12.0, 12.0),
        StateKind::History | StateKind::DeepHistory => Point::new(24.0, 24.0),
        StateKind::Choice => Point::new(32.0, 32.0),
        StateKind::Fork | StateKind::Join => Point::new(70.0, 10.0),
        StateKind::SynchroBar => Point::new(60.0, 10.0),
        StateKind::Simple | StateKind::Composite => {
            // Names may carry a literal `\n` (backslash-n, as written in
            // PlantUML source) — the painter's `_with-breaks` renders it
            // as a line break, so size for the widest line and the line
            // count, not the joined string. Mirrors states.typ's probe.
            let name_lines: Vec<&str> = n.display.split("\\n").collect();
            let name_cols = name_lines
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0);
            let name_rows = name_lines.len() as f64;
            let name_w = name_cols as f64 * CHAR_W_PT + 22.0;
            if n.body.is_empty() {
                let h = (name_rows * 13.0 + 14.0).max(32.0);
                Point::new(name_w.max(56.0), h)
            } else {
                let body_w = n
                    .body
                    .iter()
                    .map(|r| r.chars().count() as f64 * BODY_CHAR_W_PT + 16.0)
                    .fold(0.0_f64, f64::max);
                let w = name_w.max(body_w).max(64.0);
                // Name band scales with the name's line count; floor at
                // the original single-line band (26pt).
                let band = (name_rows * 13.0 + 8.0).max(26.0);
                let h = band + n.body.len() as f64 * 13.0 + 8.0;
                Point::new(w, h)
            }
        }
    };
    NodeGeom { size }
}

/// Estimate the rendered `(width, height)` (pt) of a transition's
/// `event [guard] / action` label, or `None` when it carries no label.
/// The painter renders the label at the 0.78em `_label-size` and splits
/// each part on a literal `\n`. Used both to reserve interior back-edge
/// bow room and to keep straight-edge labels inside the canvas.
fn edge_label_size(tr: &Transition) -> Option<(f64, f64)> {
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
    let joined = parts.join(" ");
    // `_with-breaks` turns a literal `\n` into a line break.
    let lines: Vec<&str> = joined.split("\\n").collect();
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let w = cols as f64 * 4.8;
    let h = lines.len() as f64 * 11.0;
    Some((w, h))
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

/// Perimeter-shape class for a node kind — mirrors `states.typ::_shape-of`.
fn node_shape(kind: StateKind) -> &'static str {
    match kind {
        StateKind::Initial
        | StateKind::Final
        | StateKind::History
        | StateKind::DeepHistory
        | StateKind::EntryPoint
        | StateKind::ExitPoint => "circle",
        StateKind::Choice => "diamond",
        _ => "rect",
    }
}

/// Clip the ray from a node centre `(cx, cy)` toward `(tx, ty)` to the
/// node's perimeter. Rust mirror of `states.typ::_perimeter` so the
/// codegen-routed endpoints land exactly where the painter would put
/// them.
fn perimeter_point(c: Point, hw: f64, hh: f64, shape: &str, toward: Point) -> Point {
    let dx = toward.x - c.x;
    let dy = toward.y - c.y;
    let adx = dx.abs();
    let ady = dy.abs();
    if adx < 1e-4 && ady < 1e-4 {
        return c;
    }
    let t = match shape {
        "circle" => {
            let r = hw.min(hh);
            let len = (adx * adx + ady * ady).sqrt();
            r / len
        }
        "diamond" => 1.0 / (adx / hw + ady / hh),
        _ => {
            let tx = if adx > 1e-4 { hw / adx } else { 1e9 };
            let ty = if ady > 1e-4 { hh / ady } else { 1e9 };
            tx.min(ty)
        }
    };
    Point::new(c.x + dx * t, c.y + dy * t)
}

/// True iff segment `a→b` enters the open interior of the axis-aligned
/// box `[lo, hi]`. Endpoints merely touching the border read as
/// outside, so an edge anchored on a box face doesn't count as
/// crossing it. Liang-Barsky parametric clip.
fn seg_crosses_box(a: Point, b: Point, lo: Point, hi: Point) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_enter = 0.0_f64;
    let mut t_exit = 1.0_f64;
    let clip = |p: f64, q: f64, t_enter: &mut f64, t_exit: &mut f64| -> bool {
        if p.abs() < 1e-9 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > *t_exit {
                return false;
            }
            if r > *t_enter {
                *t_enter = r;
            }
        } else {
            if r < *t_enter {
                return false;
            }
            if r < *t_exit {
                *t_exit = r;
            }
        }
        true
    };
    if !clip(-dx, a.x - lo.x, &mut t_enter, &mut t_exit) {
        return false;
    }
    if !clip(dx, hi.x - a.x, &mut t_enter, &mut t_exit) {
        return false;
    }
    if !clip(-dy, a.y - lo.y, &mut t_enter, &mut t_exit) {
        return false;
    }
    if !clip(dy, hi.y - a.y, &mut t_enter, &mut t_exit) {
        return false;
    }
    t_exit - t_enter > 1e-6
}

/// A routed transition: the resolved start anchor plus the cubic-bezier
/// segments `(c1, c2, end)` of the detour. `None` for transitions that
/// route as a straight line (the painter draws those itself, so no
/// emit churn for the common case).
struct RoutedEdge {
    start: Point,
    segments: Vec<(Point, Point, Point)>,
}

/// Route every "normal" transition (not self-loop, not back-edge) with
/// the obstacle-aware spline router, treating composite frames and
/// sibling leaf boxes as obstacles — the same job dot's spline router
/// does via `cl_bound` + node avoidance. Returns one slot per
/// transition; `None` means "draw straight".
///
/// Obstacle rule (mirrors dot's "cluster the edge doesn't own"): a node
/// `n` blocks edge `s→d` iff `n` is neither an ancestor-or-self nor a
/// descendant of `s` or `d`. To avoid redundant obstacles, only the
/// *outermost* blocking node is kept (a composite frame already covers
/// its interior), so a node whose parent is itself a blocker is skipped.
fn route_transitions(
    diag: &StateDiagram,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    is_lr: bool,
) -> Vec<Option<RoutedEdge>> {
    let n = diag.nodes.len();
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

    // `a` is an ancestor-or-self of `x` iff walking x's parent chain hits a.
    let anc_or_self = |a: usize, mut x: usize| -> bool {
        loop {
            if x == a {
                return true;
            }
            match parent_of[x] {
                Some(p) => x = p,
                None => return false,
            }
        }
    };
    let bbox = |i: usize| -> (Point, Point) {
        (
            top_lefts[i],
            Point::new(top_lefts[i].x + eff_geom[i].x, top_lefts[i].y + eff_geom[i].y),
        )
    };
    let center = |i: usize| -> Point {
        Point::new(
            top_lefts[i].x + eff_geom[i].x / 2.0,
            top_lefts[i].y + eff_geom[i].y / 2.0,
        )
    };

    // Phase 1: collect the transitions whose straight line is blocked.
    struct Pending {
        ti: usize,
        start: Point,
        end: Point,
        u_lo: Point,
        u_hi: Point,
    }
    let mut pending: Vec<Pending> = Vec::new();
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if tr.from == tr.to || back[ti] {
            continue;
        }
        let (s, d) = match (idx.get(tr.from.as_str()), idx.get(tr.to.as_str())) {
            (Some(&s), Some(&d)) => (s, d),
            _ => continue,
        };
        // `n` is involved with this edge (so never an obstacle) when it
        // contains or is contained by either endpoint.
        let involved = |x: usize| {
            anc_or_self(x, s) || anc_or_self(x, d) || anc_or_self(s, x) || anc_or_self(d, x)
        };
        let is_blocker = |x: usize| !involved(x);
        let start = perimeter_point(
            center(s),
            eff_geom[s].x / 2.0,
            eff_geom[s].y / 2.0,
            node_shape(diag.nodes[s].kind),
            center(d),
        );
        let end = perimeter_point(
            center(d),
            eff_geom[d].x / 2.0,
            eff_geom[d].y / 2.0,
            node_shape(diag.nodes[d].kind),
            center(s),
        );
        // Union bbox of the obstacles the straight line actually crosses.
        // Outermost blockers only (a composite frame already covers its
        // interior, so skip a node whose parent also blocks).
        let mut u_lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut u_hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut blocked = false;
        for x in 0..n {
            if !is_blocker(x) {
                continue;
            }
            if let Some(p) = parent_of[x] {
                if is_blocker(p) {
                    continue;
                }
            }
            let (lo, hi) = bbox(x);
            if seg_crosses_box(start, end, lo, hi) {
                blocked = true;
                u_lo.x = u_lo.x.min(lo.x);
                u_lo.y = u_lo.y.min(lo.y);
                u_hi.x = u_hi.x.max(hi.x);
                u_hi.y = u_hi.y.max(hi.y);
            }
        }
        if !blocked {
            continue; // straight line of sight — painter draws it
        }
        pending.push(Pending { ti, start, end, u_lo, u_hi });
    }

    // Phase 2: pick a side per detour and assign parallel lanes so
    // sibling detours don't stack on one line. Bias to the perpendicular
    // MIN side (left in TB, top in LR): self-loop arcs and back-edge bows
    // always curl onto the MAX side, so detouring on the opposite side
    // keeps the two families apart. Each successive lane sits one
    // `LANE_GAP` farther out.
    const DETOUR_MARGIN_PT: f64 = 14.0;
    const LANE_GAP_PT: f64 = 14.0;
    const SIDE_BIAS_PT: f64 = 30.0; // how much nearer right must be to win
    let mut out: Vec<Option<RoutedEdge>> = (0..diag.transitions.len()).map(|_| None).collect();
    // Lane counters per side.
    let mut lane_min = 0usize;
    let mut lane_max = 0usize;
    // Stable order: by start coord along the rank axis.
    pending.sort_by(|a, b| {
        let ka = if is_lr { a.start.x } else { a.start.y };
        let kb = if is_lr { b.start.x } else { b.start.y };
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    });
    for p in &pending {
        let (lo_side, hi_side, mid) = if is_lr {
            (
                p.u_lo.y - DETOUR_MARGIN_PT,
                p.u_hi.y + DETOUR_MARGIN_PT,
                (p.start.y + p.end.y) / 2.0,
            )
        } else {
            (
                p.u_lo.x - DETOUR_MARGIN_PT,
                p.u_hi.x + DETOUR_MARGIN_PT,
                (p.start.x + p.end.x) / 2.0,
            )
        };
        // Prefer the MIN side unless MAX is clearly nearer.
        let use_min = (mid - lo_side).abs() <= (hi_side - mid).abs() + SIDE_BIAS_PT;
        let side_coord = if use_min {
            let lane = lane_min;
            lane_min += 1;
            lo_side - lane as f64 * LANE_GAP_PT
        } else {
            let lane = lane_max;
            lane_max += 1;
            hi_side + lane as f64 * LANE_GAP_PT
        };
        let segments = detour_around(p.start, p.end, side_coord, is_lr);
        out[p.ti] = Some(RoutedEdge { start: p.start, segments });
    }
    out
}

/// A straight line `a→b` expressed as a single cubic whose control
/// handles sit at 1/3 and 2/3 — the painter draws it as the segment.
fn straight_cubic(a: Point, b: Point) -> (Point, Point, Point) {
    let c1 = Point::new(a.x + (b.x - a.x) / 3.0, a.y + (b.y - a.y) / 3.0);
    let c2 = Point::new(a.x + 2.0 * (b.x - a.x) / 3.0, a.y + 2.0 * (b.y - a.y) / 3.0);
    (c1, c2, b)
}

/// Build an axis-aligned detour from `start` to `end` whose long run sits
/// at `side_coord` on the perpendicular axis (an x in TB, a y in LR).
/// Returns three cubic segments with sharp orthogonal corners — clean and
/// unambiguous, matching PlantUML's `splines=ortho` look for routed
/// cross-edges.
fn detour_around(start: Point, end: Point, side_coord: f64, is_lr: bool) -> Vec<(Point, Point, Point)> {
    let (p1, p2) = if is_lr {
        (Point::new(start.x, side_coord), Point::new(end.x, side_coord))
    } else {
        (Point::new(side_coord, start.y), Point::new(side_coord, end.y))
    };
    vec![
        straight_cubic(start, p1),
        straight_cubic(p1, p2),
        straight_cubic(p2, end),
    ]
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
    /// Per-transition routed polyline in absolute coords (same space as
    /// `top_lefts`): `[start, ..connector-centres, end]`. Produced by the
    /// global cluster layout from each edge's connector chain (dot's
    /// "edges follow their virtual-node chain"). Empty for transitions
    /// drawn by the painter itself (self-loops, back-bows, or the
    /// per-level fallback path).
    waypoints: HashMap<usize, Vec<Point>>,
    /// Per-transition reserved label position (absolute centre), for
    /// transitions whose label was laid out as a dot-style label node.
    /// The painter draws the label here instead of computing a midpoint.
    label_pos: HashMap<usize, Point>,
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
    // rank-skipping long edge. Forward edges (including rank-skipping ones)
    // stay in the graph: dot keeps long edges as virtual-node chains so
    // the source fans out toward its edge lane, and the edge is drawn
    // along that chain. Removing them flattens the fan-out.
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

/// Global cluster layout — the dot/PlantUML model.
///
/// One `VisualGraph` holds every leaf / pseudostate as a box node;
/// composite states become `HierarchyMap` clusters (their frame is the
/// cluster bbox). Long edges stay in the graph as connector-dummy
/// chains, which the engine keeps *outside* foreign cluster boxes
/// (`graph.rs::split_long_edges` inherits the source cluster), so a
/// transition that skips past a composite routes down a lane beside it
/// — never through it. The edge is then drawn along its connector
/// chain. Edges that touch a composite are wired to an interior
/// representative (source/sink) so ranking places the composite's
/// subtree correctly.
///
/// Horizontal links (`A -> B`, single dash) condense their leaf members
/// into one super-node so they share a rank (the engine has no
/// same-rank constraint). Concurrent regions are handled by the
/// dedicated recursive path (`layout_nodes`); this function is only
/// chosen when the diagram has none.
fn layout_global(diag: &StateDiagram, base_geoms: &[NodeGeom], orientation: Orientation) -> Layout {
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
    // Derive children from `parent_of` (guaranteed consistent with the
    // cluster assignment, unlike the parser's `nd.children` which may be
    // unset for synthetic scoped pseudostates).
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        if let Some(p) = parent_of[i] {
            children_of[p].push(i);
        }
    }
    let is_comp: Vec<bool> = diag.nodes.iter().map(|nd| nd.kind == StateKind::Composite).collect();
    let mut depth = vec![0usize; n];
    for i in 0..n {
        if let Some(p) = parent_of[i] {
            depth[i] = depth[p] + 1;
        }
    }
    let all_edges: Vec<(usize, usize, usize, bool)> = diag
        .transitions
        .iter()
        .enumerate()
        .filter_map(|(ti, tr)| {
            let s = *idx.get(tr.from.as_str())?;
            let d = *idx.get(tr.to.as_str())?;
            Some((ti, s, d, tr.horizontal))
        })
        .collect();

    // --- composite representatives (deepest-first so nesting resolves) ---
    let mut in_rep = vec![None::<usize>; n];
    let mut out_rep = vec![None::<usize>; n];
    let mut comps: Vec<usize> = (0..n).filter(|&i| is_comp[i]).collect();
    comps.sort_by_key(|&c| std::cmp::Reverse(depth[c]));
    for &c in &comps {
        let kids = &children_of[c];
        if kids.is_empty() {
            continue;
        }
        let kset: HashSet<usize> = kids.iter().copied().collect();
        let mut indeg: HashMap<usize, usize> = kids.iter().map(|&k| (k, 0)).collect();
        let mut outdeg: HashMap<usize, usize> = kids.iter().map(|&k| (k, 0)).collect();
        for &(_, s, d, _) in &all_edges {
            if let (Some(ls), Some(ld)) = (lift(s, &kset, &parent_of), lift(d, &kset, &parent_of)) {
                if ls != ld {
                    *outdeg.get_mut(&ls).unwrap() += 1;
                    *indeg.get_mut(&ld).unwrap() += 1;
                }
            }
        }
        let pick = |want_source: bool| -> usize {
            let deg = if want_source { &indeg } else { &outdeg };
            let pref = if want_source { StateKind::Initial } else { StateKind::Final };
            kids.iter()
                .copied()
                .find(|&k| deg[&k] == 0 && diag.nodes[k].kind == pref)
                .or_else(|| kids.iter().copied().find(|&k| deg[&k] == 0))
                .unwrap_or(kids[0])
        };
        let src = pick(true);
        let snk = pick(false);
        in_rep[c] = Some(if is_comp[src] { in_rep[src].unwrap_or(src) } else { src });
        out_rep[c] = Some(if is_comp[snk] { out_rep[snk].unwrap_or(snk) } else { snk });
    }
    // Endpoint node for an edge end: a composite resolves to its in/out
    // representative (a non-composite descendant); a leaf is itself.
    let endpoint = |node: usize, as_source: bool| -> usize {
        if is_comp[node] {
            if as_source {
                out_rep[node].unwrap_or(node)
            } else {
                in_rep[node].unwrap_or(node)
            }
        } else {
            node
        }
    };

    // --- horizontal condensation over non-composite nodes ---
    // VG gets one node per "component"; a component is a maximal set of
    // non-composite siblings joined by horizontal links. Composites are
    // clusters, not VG nodes.
    let leaf: Vec<usize> = (0..n).filter(|&i| !is_comp[i]).collect();
    let leaf_local: HashMap<usize, usize> = leaf.iter().enumerate().map(|(l, &g)| (g, l)).collect();
    let nl = leaf.len();
    let mut uf = UnionFind::new(nl);
    for &(_, s, d, horizontal) in &all_edges {
        if horizontal && !is_comp[s] && !is_comp[d] && parent_of[s] == parent_of[d] {
            if let (Some(&ls), Some(&ld)) = (leaf_local.get(&s), leaf_local.get(&d)) {
                uf.union(ls, ld);
            }
        }
    }
    let mut comp_id: HashMap<usize, usize> = HashMap::new();
    let mut comp_members: Vec<Vec<usize>> = Vec::new(); // local leaf indices
    let comp_of_leaf: Vec<usize> = (0..nl)
        .map(|l| {
            let r = uf.find(l);
            *comp_id.entry(r).or_insert_with(|| {
                comp_members.push(Vec::new());
                comp_members.len() - 1
            })
        })
        .collect();
    for l in 0..nl {
        comp_members[comp_of_leaf[l]].push(l);
    }
    // node (global) → component index
    let comp_of_node = |g: usize| -> Option<usize> { leaf_local.get(&g).map(|&l| comp_of_leaf[l]) };

    // Horizontal adjacency for ordering within a component (leaf-local).
    let mut horiz_adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for &(_, s, d, horizontal) in &all_edges {
        if horizontal && !is_comp[s] && !is_comp[d] {
            if let (Some(&ls), Some(&ld)) = (leaf_local.get(&s), leaf_local.get(&d)) {
                if seen.insert((ls, ld)) {
                    horiz_adj.entry(ls).or_default().push(ld);
                }
            }
        }
    }

    // Side-by-side layout of each component's members; record member
    // offset (within the component box) + component box size.
    let (pad_rank, pad_perp) = (LAYOUT_PAD_Y_PT, LAYOUT_PAD_X_PT);
    let lgeom = |l: usize| base_geoms[leaf[l]].size;
    let mut member_off: Vec<Point> = vec![Point::zero(); nl];
    let mut comp_size: Vec<Point> = vec![Point::zero(); comp_members.len()];
    for (cid, members) in comp_members.iter().enumerate() {
        let ordered = order_component(members, &horiz_adj);
        let span = ordered
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
                member_off[l] = Point::new((span - g.x) / 2.0, cursor);
                cursor += g.y;
            } else {
                member_off[l] = Point::new(cursor, (span - g.y) / 2.0);
                cursor += g.x;
            }
        }
        comp_size[cid] = if is_lr {
            Point::new(span, cursor)
        } else {
            Point::new(cursor, span)
        };
    }

    // --- build the VisualGraph + cluster hierarchy ---
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

    // Cluster per composite; map composite index → ClusterId.
    let mut hierarchy = crate::layout::sugiyama::HierarchyMap::new();
    let mut cluster_of_comp: HashMap<usize, usize> = HashMap::new();
    // Stable order: shallowest-first so parents exist before children.
    let mut comp_by_depth: Vec<usize> = comps.clone();
    comp_by_depth.sort_by_key(|&c| depth[c]);
    for &c in &comp_by_depth {
        let cid = hierarchy.add_cluster(None);
        cluster_of_comp.insert(c, cid);
        hierarchy.clusters[cid].pad = COMPOSITE_PAD_PT;
        hierarchy.clusters[cid].label_band = COMPOSITE_LABEL_BAND_PT;
        hierarchy.clusters[cid].label_min_w = base_geoms[c].size.x;
    }
    // Wire cluster parents.
    for &c in &comp_by_depth {
        let cid = cluster_of_comp[&c];
        if let Some(p) = parent_of[c] {
            if let Some(&pcid) = cluster_of_comp.get(&p) {
                hierarchy.clusters[cid].parent = Some(pcid);
                hierarchy.clusters[pcid].direct_children.push(cid);
            }
        }
    }
    // Assign each component super-node to the innermost composite that
    // owns its members (members share a parent).
    for (cid, members) in comp_members.iter().enumerate() {
        let any = leaf[members[0]];
        if let Some(p) = parent_of[any] {
            if let Some(&clid) = cluster_of_comp.get(&p) {
                hierarchy.assign_node(comp_handles[cid], clid);
            }
        }
    }
    vg.set_hierarchy(hierarchy);

    // Cluster id of each component (for assigning label nodes so they
    // don't break cluster contiguity — mirrors how connector dummies
    // inherit their source's cluster).
    let comp_cluster: Vec<Option<usize>> = comp_members
        .iter()
        .map(|members| {
            let any = leaf[members[0]];
            parent_of[any].and_then(|p| cluster_of_comp.get(&p).copied())
        })
        .collect();

    // Edges: one VG edge per transition (skip self-loops + dangling),
    // wired through composite representatives + component super-nodes.
    // A labelled transition gets a **label node** sized to its text
    // inserted between source and target (dot's edge-label virtual node):
    // it reserves a rank + perpendicular space so the diagram spreads to
    // fit the labels and each label owns a clear position on its edge.
    let mut edge_vg: Vec<Option<(usize, usize)>> = vec![None; diag.transitions.len()];
    let mut label_handle: Vec<Option<NodeHandle>> = vec![None; diag.transitions.len()];
    // Component-graph reachability, built in transition order to mirror the
    // engine's `normalize_dag`: an edge whose target can already reach its
    // source is a back-edge (cycle). We must NOT give back-edges a label
    // node — a label node on a reversed edge scrambles the cluster/rank
    // layout; the painter draws those labels on its bow instead.
    let mut comp_adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let reaches = |adj: &HashMap<usize, Vec<usize>>, from: usize, to: usize| -> bool {
        let mut stack = vec![from];
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(x) = stack.pop() {
            if x == to {
                return true;
            }
            if !seen.insert(x) {
                continue;
            }
            if let Some(ns) = adj.get(&x) {
                stack.extend(ns.iter().copied());
            }
        }
        false
    };
    for &(ti, s, d, horizontal) in &all_edges {
        if s == d {
            continue;
        }
        let es = endpoint(s, true);
        let ed = endpoint(d, false);
        let (cs, cd) = match (comp_of_node(es), comp_of_node(ed)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        if cs == cd {
            continue;
        }
        edge_vg[ti] = Some((cs, cd));
        let is_back = reaches(&comp_adj, cd, cs);
        if !is_back {
            comp_adj.entry(cs).or_default().push(cd);
        }
        // Label nodes are a flat-graph device (they reserve a rank +
        // perpendicular slot, dot-style). Inside / across composite frames
        // they fight cluster contiguity and the frame-clearance passes, so
        // restrict them to top-level edges whose both ends sit outside any
        // composite. Edges touching a composite keep the painter's own
        // label placement, which the cluster path already lays out well.
        let both_top = comp_cluster[cs].is_none() && comp_cluster[cd].is_none();
        let want_label = !(horizontal || is_back) && both_top;
        match (edge_label_size(&diag.transitions[ti]), want_label) {
            (Some((lw, lh_h)), true) => {
                let lh = vg.add_node(Element::new_box(Point::new(lw, lh_h), orientation));
                vg.add_edge(Edge::default(), comp_handles[cs], lh);
                vg.add_edge(Edge::default(), lh, comp_handles[cd]);
                label_handle[ti] = Some(lh);
            }
            _ => {
                vg.add_edge(Edge::default(), comp_handles[cs], comp_handles[cd]);
            }
        }
    }

    vg.layout();

    // --- extract node positions ---
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
    let mut top_lefts = vec![Point::zero(); n];
    for l in 0..nl {
        let c = comp_topleft[comp_of_leaf[l]];
        let off = member_off[l];
        top_lefts[leaf[l]] = Point::new(c.x + off.x, c.y + off.y);
    }

    // Composite frames from cluster bboxes.
    let mut eff_geom: Vec<Point> = base_geoms.iter().map(|g| g.size).collect();
    for &c in &comps {
        if let Some(&cid) = cluster_of_comp.get(&c) {
            let cl = &vg.hierarchy.clusters[cid];
            if cl.x_min.is_finite() && cl.x_max.is_finite() {
                top_lefts[c] = Point::new(cl.x_min, cl.y_min);
                eff_geom[c] = Point::new(cl.x_max - cl.x_min, cl.y_max - cl.y_min);
            }
        }
    }

    // --- per-transition waypoints + label positions ---
    // A labelled edge was split into source→label and label→target, so
    // match the lowered edges by their endpoint handles (not insertion
    // order) and stitch each transition's path. The label node centre is
    // both a routing waypoint and the label's reserved position.
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
            (chain.first().unwrap().get_index(), chain.last().unwrap().get_index()),
            pts,
        );
    }
    // Oriented connector mids for segment a→b (drops the two endpoints).
    let seg_mids = |a: NodeHandle, b: NodeHandle| -> (Vec<Point>, bool) {
        let mids = |pts: &Vec<Point>| -> Vec<Point> {
            if pts.len() > 2 {
                pts[1..pts.len() - 1].to_vec()
            } else {
                Vec::new()
            }
        };
        if let Some(pts) = chain_of.get(&(a.get_index(), b.get_index())) {
            (mids(pts), false)
        } else if let Some(pts) = chain_of.get(&(b.get_index(), a.get_index())) {
            let mut m = mids(pts);
            m.reverse();
            (m, true)
        } else {
            (Vec::new(), false)
        }
    };
    let mut back = vec![false; diag.transitions.len()];
    let mut waypoints: HashMap<usize, Vec<Point>> = HashMap::new();
    let mut label_pos: HashMap<usize, Point> = HashMap::new();
    for ti in 0..diag.transitions.len() {
        let Some((cs, cd)) = edge_vg[ti] else { continue };
        let (sh, th) = (comp_handles[cs], comp_handles[cd]);
        if let Some(lh) = label_handle[ti] {
            let lc = center(lh);
            label_pos.insert(ti, lc);
            let (mut mids, _) = seg_mids(sh, lh);
            mids.push(lc);
            let (tail, _) = seg_mids(lh, th);
            mids.extend(tail);
            waypoints.insert(ti, mids);
        } else {
            let (mids, reversed) = seg_mids(sh, th);
            if reversed {
                back[ti] = true;
            }
            if !mids.is_empty() {
                waypoints.insert(ti, mids);
            }
        }
    }

    let inside = |mut node: usize, c: usize| -> bool {
        loop {
            if node == c {
                return true;
            }
            match parent_of[node] {
                Some(p) => node = p,
                None => return false,
            }
        }
    };

    // --- frame-exit clearance -----------------------------------------
    // When an edge leaves a composite frame to a node below it, that node
    // should sit a rank gap below the *frame bottom*, not hard against the
    // composite's lower border (a terminal/sink otherwise hangs on the
    // edge). This is the bottom-side mirror of the title-band clearance
    // `restore_rank_monotonicity` does at the top — but it lives here, not
    // in the shared `tighten`, because it must add room even when the node
    // already barely clears the frame, which would churn cuca packages.
    // Push such targets down and relax the shift through their successors
    // so rank monotonicity is preserved.
    {
        const FRAME_EXIT_GAP: f64 = 16.0;
        // Operate on the *rank* axis (x for left-to-right, y for top-to-
        // bottom): successors flow along it, so "below the frame" means
        // "further along the rank axis", not literally down in y.
        let rank_lo = |tl: &[Point], i: usize| if is_lr { tl[i].x } else { tl[i].y };
        let rank_hi =
            |tl: &[Point], i: usize| if is_lr { tl[i].x + eff_geom[i].x } else { tl[i].y + eff_geom[i].y };
        let mut moved: HashSet<usize> = HashSet::new();
        for _ in 0..(n + 2) {
            let mut changed = false;
            for &(ti, s, d, horizontal) in &all_edges {
                // Skip self-loops, back-edges, and horizontal links. A
                // back-edge runs against the rank flow (cycles like
                // Red→Green→Yellow→Red); a horizontal `->` link places its
                // ends side by side on the *same* rank. Treating either as
                // a forward rank constraint would scramble the layout.
                if s == d || back[ti] || horizontal {
                    continue;
                }
                let mut req = rank_hi(&top_lefts, s); // keep d past s along rank
                for &c in &comps {
                    if inside(s, c) && !inside(d, c) {
                        req = req.max(rank_hi(&top_lefts, c) + FRAME_EXIT_GAP);
                    }
                }
                let delta = req - rank_lo(&top_lefts, d);
                if delta > 1e-6 {
                    for i in 0..n {
                        if i == d || inside(i, d) {
                            if is_lr {
                                top_lefts[i].x += delta;
                            } else {
                                top_lefts[i].y += delta;
                            }
                            moved.insert(i);
                        }
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        if !moved.is_empty() {
            for &(ti, s, d, _) in &all_edges {
                if moved.contains(&s) || moved.contains(&d) {
                    waypoints.remove(&ti);
                }
            }
        }
    }

    // --- perp-axis centering ------------------------------------------
    // Straighten chains and centre a fork's parent over its children:
    // pull each node that is *alone in its rank* toward the average perp
    // coordinate of its graph neighbours. The rank axis is x (left-to-
    // right) or y (top-to-bottom); the perp axis is the other. A fork's
    // children share a rank, so they're held at the engine's symmetric
    // placement and the parent settles on their midpoint — giving the
    // straight spine + symmetric branch dot produces.
    {
        let is_perp_x = !is_lr;
        let pc = |tl: &[Point], i: usize| {
            if is_perp_x {
                tl[i].x + eff_geom[i].x / 2.0
            } else {
                tl[i].y + eff_geom[i].y / 2.0
            }
        };
        let rlo = |tl: &[Point], i: usize| if is_lr { tl[i].x } else { tl[i].y };
        let rhi =
            |tl: &[Point], i: usize| if is_lr { tl[i].x + eff_geom[i].x } else { tl[i].y + eff_geom[i].y };
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(_, s, d, _) in &all_edges {
            if s != d {
                adj[s].push(d);
                adj[d].push(s);
            }
        }
        let movable: Vec<usize> = (0..n)
            .filter(|&i| parent_of[i].is_none() && !is_comp[i] && !adj[i].is_empty())
            .filter(|&i| {
                let (lo, hi) = (rlo(&top_lefts, i), rhi(&top_lefts, i));
                !(0..n).any(|j| {
                    j != i
                        && parent_of[j].is_none()
                        && rhi(&top_lefts, j) > lo + 1.0
                        && rlo(&top_lefts, j) < hi - 1.0
                })
            })
            .collect();
        let mut moved: HashSet<usize> = HashSet::new();
        for _ in 0..12 {
            for &i in &movable {
                let desired =
                    adj[i].iter().map(|&j| pc(&top_lefts, j)).sum::<f64>() / adj[i].len() as f64;
                let delta = desired - pc(&top_lefts, i);
                if delta.abs() > 1e-6 {
                    if is_perp_x {
                        top_lefts[i].x += delta;
                    } else {
                        top_lefts[i].y += delta;
                    }
                    moved.insert(i);
                }
            }
        }
        if !moved.is_empty() {
            for &(ti, s, d, _) in &all_edges {
                if moved.contains(&s) || moved.contains(&d) {
                    waypoints.remove(&ti);
                }
            }
        }
    }

    // --- dot-style composite offset ----------------------------------
    // Brandes-Köpf centres the State1→State2→State3→terminal chain in one
    // column, so a shared terminal lands directly under the composite and
    // the "bypass" edges (a sibling above the composite going to the
    // terminal below it) get squeezed into lanes beside / through it. dot
    // instead pushes the composite to one side and keeps the bypass
    // sources + terminal in a clear column on the other. Replicate that
    // here (state-only; cuca/records never take this path): when a
    // top-level composite has bypass traffic, align the sink(s) to the
    // bypass column and slide the composite subtree clear of it.
    // Geometry below is written for top-to-bottom flow (rank = y); skip
    // it for left-to-right diagrams rather than mis-apply the axes.
    let cx = |tl: &[Point], i: usize| tl[i].x + eff_geom[i].x / 2.0;
    for &c in &comps {
        if is_lr {
            break;
        }
        if parent_of[c].is_some() {
            continue; // top-level composites only
        }
        let c_top = top_lefts[c].y;
        let c_bot = top_lefts[c].y + eff_geom[c].y;
        let (c_l, c_r) = (top_lefts[c].x, top_lefts[c].x + eff_geom[c].x);
        // Collect bypass edges: endpoints both outside C, one above C and
        // one below it, whose span crosses C's column.
        let mut sources: Vec<usize> = Vec::new();
        let mut sinks: Vec<usize> = Vec::new();
        for &(_, s, d, _) in &all_edges {
            if s == d || inside(s, c) || inside(d, c) {
                continue;
            }
            let sy = top_lefts[s].y + eff_geom[s].y / 2.0;
            let dy = top_lefts[d].y + eff_geom[d].y / 2.0;
            let (above, below) = if sy < dy { (s, d) } else { (d, s) };
            let crosses_y = top_lefts[above].y + eff_geom[above].y <= c_top + 1.0
                && top_lefts[below].y >= c_bot - 1.0;
            if !crosses_y {
                continue;
            }
            let (ax, bx) = (cx(&top_lefts, above), cx(&top_lefts, below));
            if ax.min(bx) <= c_r && ax.max(bx) >= c_l {
                sources.push(above);
                sinks.push(below);
            }
        }
        if sinks.is_empty() {
            continue;
        }
        let mut col: Vec<f64> = sources.iter().map(|&i| cx(&top_lefts, i)).collect();
        col.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let col_x = col[col.len() / 2];
        let go_left = col_x >= (c_l + c_r) / 2.0;
        // Align top-level sinks onto the bypass column.
        let mut moved: HashSet<usize> = HashSet::new();
        for &t in &sinks {
            if parent_of[t].is_some() {
                continue;
            }
            let dx = col_x - cx(&top_lefts, t);
            if dx.abs() > f64::EPSILON {
                top_lefts[t].x += dx;
                moved.insert(t);
            }
        }
        // Slide C's subtree clear of the column on the chosen side.
        const LANE: f64 = 14.0;
        let col_l = sources
            .iter()
            .chain(sinks.iter())
            .map(|&i| top_lefts[i].x)
            .fold(f64::INFINITY, f64::min);
        let col_r = sources
            .iter()
            .chain(sinks.iter())
            .map(|&i| top_lefts[i].x + eff_geom[i].x)
            .fold(f64::NEG_INFINITY, f64::max);
        let shift = if go_left {
            (c_r - (col_l - LANE)).max(0.0).copysign(-1.0)
        } else {
            (col_r + LANE - c_l).max(0.0)
        };
        if shift.abs() > f64::EPSILON {
            for i in 0..n {
                if inside(i, c) {
                    top_lefts[i].x += shift;
                    moved.insert(i);
                }
            }
        }
        // Any edge touching a moved node has stale waypoints — drop them
        // so the emit re-routes from final positions (straight or detour).
        if !moved.is_empty() {
            for &(ti, s, d, _) in &all_edges {
                if moved.contains(&s) || moved.contains(&d) {
                    waypoints.remove(&ti);
                }
            }
        }
    }

    // --- entry/exit points on the composite border -------------------
    // PlantUML draws <<entryPoint>>/<<exitPoint>> straddling the frame
    // edge (entry on the inflow edge, exit on the outflow edge), centred
    // on the border line — not as interior nodes. Recompute each owning
    // frame to exclude them, then snap them onto the edge, aligned with
    // the interior node they connect to.
    {
        let is_pt =
            |i: usize| matches!(diag.nodes[i].kind, StateKind::EntryPoint | StateKind::ExitPoint);
        // Rank-axis low / high edge of node `i` (x for LR, y for TB).
        let rank_lo = |tl: &[Point], i: usize| if is_lr { tl[i].x } else { tl[i].y };
        let rank_hi =
            |tl: &[Point], g: &[Point], i: usize| rank_lo(tl, i) + if is_lr { g[i].x } else { g[i].y };
        let perp_c =
            |tl: &[Point], g: &[Point], i: usize| if is_lr { tl[i].y + g[i].y / 2.0 } else { tl[i].x + g[i].x / 2.0 };
        let mut moved: HashSet<usize> = HashSet::new();
        for &c in &comps {
            let points: Vec<usize> =
                (0..n).filter(|&i| parent_of[i] == Some(c) && is_pt(i)).collect();
            let interior: Vec<usize> =
                (0..n).filter(|&i| i != c && inside(i, c) && !is_pt(i)).collect();
            if points.is_empty() || interior.is_empty() {
                continue;
            }
            // Recompute the frame on the rank axis to exclude the points.
            // The title band sits on the inflow (lo) side, but only in TB.
            let lo_inset = COMPOSITE_PAD_PT + if is_lr { 0.0 } else { COMPOSITE_LABEL_BAND_PT };
            let lo = interior.iter().map(|&i| rank_lo(&top_lefts, i)).fold(f64::INFINITY, f64::min)
                - lo_inset;
            let hi = interior
                .iter()
                .map(|&i| rank_hi(&top_lefts, &eff_geom, i))
                .fold(f64::NEG_INFINITY, f64::max)
                + COMPOSITE_PAD_PT;
            if is_lr {
                top_lefts[c].x = lo;
                eff_geom[c].x = hi - lo;
            } else {
                top_lefts[c].y = lo;
                eff_geom[c].y = hi - lo;
            }
            for &pt in &points {
                // Entry sits on the inflow edge aligned to its successor;
                // exit on the outflow edge aligned to its predecessor.
                let entry = diag.nodes[pt].kind == StateKind::EntryPoint;
                let conn = diag.transitions.iter().find_map(|t| {
                    if entry && t.from == diag.nodes[pt].id {
                        idx.get(t.to.as_str()).copied()
                    } else if !entry && t.to == diag.nodes[pt].id {
                        idx.get(t.from.as_str()).copied()
                    } else {
                        None
                    }
                });
                let perp = perp_c(&top_lefts, &eff_geom, conn.unwrap_or(c));
                let edge = if entry { lo } else { hi };
                top_lefts[pt] = if is_lr {
                    Point::new(edge - eff_geom[pt].x / 2.0, perp - eff_geom[pt].y / 2.0)
                } else {
                    Point::new(perp - eff_geom[pt].x / 2.0, edge - eff_geom[pt].y / 2.0)
                };
                moved.insert(pt);
            }
        }
        if !moved.is_empty() {
            for &(ti, s, d, _) in &all_edges {
                if moved.contains(&s) || moved.contains(&d) {
                    waypoints.remove(&ti);
                }
            }
        }
    }

    Layout {
        top_lefts,
        eff_geom,
        back,
        dividers: vec![Vec::new(); n],
        waypoints,
        label_pos,
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
                let need = match edge_label_size(&diag.transitions[ti]) {
                    // bow apex (`bow + 3pt`) + label width + margin.
                    Some((w, _)) => 33.0 + w + 6.0,
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

        // Floor the frame width to the label box (measured when available).
        let label_w = base_geoms[c].size.x;
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
        waypoints: HashMap::new(),
        label_pos: HashMap::new(),
    }
}

pub fn emit(
    out: &mut String,
    diag: &StateDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    let _ = &diag.skinparams; // S4: skinparam state* → preamble

    if diag.nodes.is_empty() {
        out.push_str("#state-layout()\n");
        return;
    }

    let geoms: Vec<NodeGeom> = diag
        .nodes
        .iter()
        .map(|n| resolve_node_geom(n, measurements, diagram_idx))
        .collect();

    let orientation = match diag.direction {
        LayoutDirection::TopToBottom => Orientation::TopToBottom,
        LayoutDirection::LeftToRight => Orientation::LeftToRight,
    };

    let id_to_idx = |id: &str| diag.nodes.iter().position(|n| n.id == id);

    // `layout_global` (dot-style global cluster layout) handles flat
    // diagrams, single-level and nested composites: leaves/pseudostates
    // are box nodes, composites are HierarchyMap clusters, long edges
    // route down lanes beside the composite like dot, sources fan out,
    // the final sits at the bottom, and `tighten::restore_rank_monotonicity`
    // keeps a composite's outside successors below its frame. Concurrent
    // regions still need the dedicated recursive layout. See
    // docs/state-routing-redesign.md.
    let use_global = diag.regions.is_empty();
    let layout = if use_global {
        layout_global(diag, &geoms, orientation)
    } else {
        layout_nodes(diag, &geoms, orientation)
    };
    let mut top_lefts = layout.top_lefts;
    let eff_geom = layout.eff_geom;
    let back = layout.back;
    let dividers = layout.dividers;
    let mut waypoints = layout.waypoints;
    let mut label_pos = layout.label_pos;
    let is_lr = matches!(orientation, Orientation::LeftToRight);

    // Normalize so the content starts at (MARGIN, MARGIN).
    let min_x = top_lefts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = top_lefts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    for p in &mut top_lefts {
        p.x = p.x - min_x + MARGIN_PT;
        p.y = p.y - min_y + MARGIN_PT;
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            p.x = p.x - min_x + MARGIN_PT;
            p.y = p.y - min_y + MARGIN_PT;
        }
    }
    for p in label_pos.values_mut() {
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
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            if is_lr {
                p.y += reserve_lo;
            } else {
                p.x += reserve_lo;
            }
        }
    }
    for p in label_pos.values_mut() {
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
    // Stacking cursor for unconnected floating notes (placed in a left
    // column; content re-normalization shifts the column to the margin).
    let mut float_cursor_y = MARGIN_PT;
    for (note_idx, note) in diag.notes.iter().enumerate() {
        match &note.anchor {
            NoteAnchor::OfNode { node_id, side } => {
                let Some(ai) = id_to_idx(node_id) else { continue };
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
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
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
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
            NoteAnchor::Floating { links, .. } => {
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
                match links.iter().find_map(|id| id_to_idx(id)) {
                    // Connected: place like a right-of note next to the
                    // first linked state, with a dashed connector.
                    Some(ai) => {
                        let ap = top_lefts[ai];
                        let ag = eff_geom[ai];
                        let cy = ap.y + ag.y / 2.0 - sz.y / 2.0;
                        let nx =
                            clear_note_x(ap.x + ag.x + NOTE_GAP_PT, cy, sz.x, sz.y, "right");
                        note_boxes.push(NoteBox {
                            x: nx,
                            y: cy,
                            w: sz.x,
                            h: sz.y,
                            body: note.body.clone(),
                            side: "right",
                            ax: ap.x,
                            ay: ap.y,
                            aw: ag.x,
                            ah: ag.y,
                        });
                    }
                    // Unconnected: stack in a left column, no connector.
                    None => {
                        let y = float_cursor_y;
                        float_cursor_y += sz.y + 10.0;
                        note_boxes.push(NoteBox {
                            x: -sz.x - NOTE_GAP_PT,
                            y,
                            w: sz.x,
                            h: sz.y,
                            body: note.body.clone(),
                            side: "none",
                            ax: 0.0,
                            ay: 0.0,
                            aw: 0.0,
                            ah: 0.0,
                        });
                    }
                }
            }
        }
    }

    // Estimate straight-edge label boxes (the painter offsets each label
    // perpendicular to its edge) so a label on a left-column edge isn't
    // clipped off the canvas. Self-loop / back-edge labels live inside the
    // reserved bow bands and are already covered by `reserve_*`.
    let mut label_boxes: Vec<(f64, f64, f64, f64)> = Vec::new();
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if tr.from == tr.to || back[ti] {
            continue;
        }
        let (Some(s), Some(d)) = (id_to_idx(&tr.from), id_to_idx(&tr.to)) else {
            continue;
        };
        let Some((w, h)) = edge_label_size(tr) else {
            continue;
        };
        // A label laid out as a dot-style label node has a reserved
        // position; the painter draws it just right of that point. Box it
        // there. Otherwise fall back to the perpendicular-midpoint estimate.
        if let Some(p) = label_pos.get(&ti) {
            label_boxes.push((p.x, p.y - h / 2.0, w, h));
            continue;
        }
        let (sc, dc) = (center(s), center(d));
        let (mx, my) = ((sc.x + dc.x) / 2.0, (sc.y + dc.y) / 2.0);
        let (dx, dy) = (dc.x - sc.x, dc.y - sc.y);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if len > 1e-6 {
            (-dy / len, dx / len)
        } else {
            (0.0, -1.0)
        };
        let off = nx.abs() * w / 2.0 + ny.abs() * h / 2.0 + 4.0;
        let (lcx, lcy) = (mx + nx * off, my + ny * off);
        label_boxes.push((lcx - w / 2.0, lcy - h / 2.0, w, h));
    }

    // A left-of note (or a `"min"` bow on a left-edge node, or a left-side
    // edge label) may have pushed content past x = 0 / y = 0. Re-normalize.
    let content_min_x = top_lefts
        .iter()
        .map(|p| p.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x, n.ax]))
        .chain(label_boxes.iter().map(|l| l.0))
        .fold(f64::INFINITY, f64::min);
    let content_min_y = top_lefts
        .iter()
        .map(|p| p.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y, n.ay]))
        .chain(label_boxes.iter().map(|l| l.1))
        .fold(f64::INFINITY, f64::min);
    let shift_x = (MARGIN_PT - content_min_x).max(0.0);
    let shift_y = (MARGIN_PT - content_min_y).max(0.0);
    for p in &mut top_lefts {
        p.x += shift_x;
        p.y += shift_y;
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            p.x += shift_x;
            p.y += shift_y;
        }
    }
    for p in label_pos.values_mut() {
        p.x += shift_x;
        p.y += shift_y;
    }
    for nb in &mut note_boxes {
        nb.x += shift_x;
        nb.y += shift_y;
        nb.ax += shift_x;
        nb.ay += shift_y;
    }
    for lb in &mut label_boxes {
        lb.0 += shift_x;
        lb.1 += shift_y;
    }

    // Per-transition routed path. The global cluster layout supplies
    // connector waypoints (dot's "edge follows its virtual-node chain");
    // we clip the real endpoints to the node faces and stitch the chain
    // into cubic segments. Diagrams on the recursive (concurrent) path
    // have no waypoints and fall back to the obstacle detour router.
    let id_pos = |id: &str| id_to_idx(id);
    let node_center = |i: usize| -> Point {
        Point::new(
            top_lefts[i].x + eff_geom[i].x / 2.0,
            top_lefts[i].y + eff_geom[i].y / 2.0,
        )
    };
    let mut routed: Vec<Option<RoutedEdge>> = if waypoints.is_empty() {
        route_transitions(diag, &top_lefts, &eff_geom, &back, is_lr)
    } else {
        (0..diag.transitions.len()).map(|_| None).collect()
    };
    if !waypoints.is_empty() {
        // Composite frames are routing obstacles: an edge between two
        // states that both live *outside* a composite must skirt its
        // frame, not cut diagonally through it. The connector waypoints
        // alone don't guarantee this — compaction bunches the side-lane
        // dummies near the top, leaving a long straight tail that pierces
        // the box — so detect a crossing and replace the path with a
        // clean ortho detour down a side lane (dot's `splines=ortho`
        // look). Parallel detours around the same side get spread into
        // distinct lanes.
        const ROUTE_LANE_GAP: f64 = 14.0;
        let comp_boxes: Vec<(usize, Point, Point)> = diag
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == StateKind::Composite)
            .map(|(i, _)| {
                (
                    i,
                    top_lefts[i],
                    Point::new(top_lefts[i].x + eff_geom[i].x, top_lefts[i].y + eff_geom[i].y),
                )
            })
            .collect();
        let is_descendant = |mut node: usize, comp: usize| -> bool {
            loop {
                if node == comp {
                    return true;
                }
                match diag.nodes[node].parent.as_deref().and_then(id_to_idx) {
                    Some(p) => node = p,
                    None => return false,
                }
            }
        };
        let mut lane_count: HashMap<(usize, bool), usize> = HashMap::new();
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (id_pos(&tr.from), id_pos(&tr.to)) else { continue };
            if s == d {
                continue;
            }
            let mids = waypoints.get(&ti);
            let toward_s = mids.and_then(|m| m.first().copied()).unwrap_or_else(|| node_center(d));
            let toward_d = mids.and_then(|m| m.last().copied()).unwrap_or_else(|| node_center(s));
            let start = perimeter_point(
                node_center(s),
                eff_geom[s].x / 2.0,
                eff_geom[s].y / 2.0,
                node_shape(diag.nodes[s].kind),
                toward_s,
            );
            let end = perimeter_point(
                node_center(d),
                eff_geom[d].x / 2.0,
                eff_geom[d].y / 2.0,
                node_shape(diag.nodes[d].kind),
                toward_d,
            );
            let obstacle = comp_boxes.iter().find(|(ci, lo, hi)| {
                !is_descendant(s, *ci)
                    && !is_descendant(d, *ci)
                    && seg_crosses_box(
                        start,
                        end,
                        Point::new(lo.x - 1.0, lo.y - 1.0),
                        Point::new(hi.x + 1.0, hi.y + 1.0),
                    )
            });
            if let Some((ci, lo, hi)) = obstacle {
                let side_hi = if is_lr {
                    (start.y + end.y) / 2.0 >= (lo.y + hi.y) / 2.0
                } else {
                    (start.x + end.x) / 2.0 >= (lo.x + hi.x) / 2.0
                };
                let k = {
                    let e = lane_count.entry((*ci, side_hi)).or_insert(0);
                    let v = *e;
                    *e += 1;
                    v
                };
                let off = ROUTE_LANE_GAP * (1.0 + k as f64);
                let side_coord = match (is_lr, side_hi) {
                    (false, true) => hi.x + off,
                    (false, false) => lo.x - off,
                    (true, true) => hi.y + off,
                    (true, false) => lo.y - off,
                };
                let segments = detour_around(start, end, side_coord, is_lr);
                routed[ti] = Some(RoutedEdge { start, segments });
            } else if let Some(mids) = mids {
                if mids.is_empty() {
                    continue;
                }
                let mut pts = Vec::with_capacity(mids.len() + 2);
                pts.push(start);
                pts.extend_from_slice(mids);
                pts.push(end);
                let segments: Vec<(Point, Point, Point)> =
                    pts.windows(2).map(|w| straight_cubic(w[0], w[1])).collect();
                routed[ti] = Some(RoutedEdge { start, segments });
            }
        }
    }

    // Port assignment. dot routes every edge through its own virtual-node
    // lane, so several edges incident on one node face leave/enter at
    // *distinct points* spread along that face (ordered toward their far
    // ends) rather than one shared perimeter point. That is why State3's
    // two exits to the terminal leave from the bottom-right corner and the
    // bottom-middle, not as one coincident line. Replicate it generally:
    // group each non-detoured edge endpoint by (node, face), and for any
    // face carrying >= 2 edges, fan the ports around the natural exit
    // direction and route a smooth spline that leaves each box outward.
    {
        struct Ep {
            ti: usize,
            node: usize,
            is_src: bool,
            other: Point,
        }
        let face_of = |node: usize, other: Point| -> u8 {
            let c = node_center(node);
            let (dx, dy) = (other.x - c.x, other.y - c.y);
            if dy.abs() >= dx.abs() {
                if dy >= 0.0 { 1 } else { 0 } // bottom / top
            } else if dx >= 0.0 {
                3 // right
            } else {
                2 // left
            }
        };
        let mut eps: Vec<Ep> = Vec::new();
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (id_pos(&tr.from), id_pos(&tr.to)) else { continue };
            if s == d || back[ti] || routed[ti].is_some() {
                continue;
            }
            eps.push(Ep { ti, node: s, is_src: true, other: node_center(d) });
            eps.push(Ep { ti, node: d, is_src: false, other: node_center(s) });
        }
        let mut groups: HashMap<(usize, u8), Vec<usize>> = HashMap::new();
        for (i, ep) in eps.iter().enumerate() {
            groups.entry((ep.node, face_of(ep.node, ep.other))).or_default().push(i);
        }
        let mut port: HashMap<(usize, bool), Point> = HashMap::new();
        for ((node, face), mut idxs) in groups {
            if idxs.len() < 2 {
                continue; // single-edge faces keep the painter's perimeter point
            }
            let c = node_center(node);
            let (hw, hh) = (eff_geom[node].x / 2.0, eff_geom[node].y / 2.0);
            let shape = node_shape(diag.nodes[node].kind);
            let horiz = face <= 1; // top/bottom spread along x; left/right along y
            let along = |p: Point| if horiz { p.x } else { p.y };
            idxs.sort_by(|&a, &b| {
                along(eps[a].other)
                    .partial_cmp(&along(eps[b].other))
                    .unwrap()
                    .then(eps[a].ti.cmp(&eps[b].ti))
            });
            let count = idxs.len();
            let centroid = idxs
                .iter()
                .map(|&i| along(perimeter_point(c, hw, hh, shape, eps[i].other)))
                .sum::<f64>()
                / count as f64;
            let (lo, hi) = if horiz {
                (c.x - hw + 4.0, c.x + hw - 4.0)
            } else {
                (c.y - hh + 4.0, c.y + hh - 4.0)
            };
            let spacing = ((hi - lo) / count as f64).clamp(6.0, 16.0);
            for (k, &i) in idxs.iter().enumerate() {
                let pos = (centroid + (k as f64 - (count as f64 - 1.0) / 2.0) * spacing).clamp(lo, hi);
                let pt = match face {
                    0 => Point::new(pos, c.y - hh),
                    1 => Point::new(pos, c.y + hh),
                    2 => Point::new(c.x - hw, pos),
                    _ => Point::new(c.x + hw, pos),
                };
                port.insert((eps[i].ti, eps[i].is_src), pt);
            }
        }
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (id_pos(&tr.from), id_pos(&tr.to)) else { continue };
            if s == d || back[ti] || routed[ti].is_some() {
                continue;
            }
            let (ps, pd) = (port.get(&(ti, true)).copied(), port.get(&(ti, false)).copied());
            if ps.is_none() && pd.is_none() {
                continue; // no multi-edge face on either end
            }
            let (cs, cd) = (node_center(s), node_center(d));
            let start = ps.unwrap_or_else(|| {
                perimeter_point(cs, eff_geom[s].x / 2.0, eff_geom[s].y / 2.0, node_shape(diag.nodes[s].kind), cd)
            });
            let end = pd.unwrap_or_else(|| {
                perimeter_point(cd, eff_geom[d].x / 2.0, eff_geom[d].y / 2.0, node_shape(diag.nodes[d].kind), cs)
            });
            // The distinct port already separates this edge from its
            // siblings, so draw a straight line — a curved spline would
            // bow an edge that should run straight (fork bar → worker,
            // worker → join bar, etc.).
            routed[ti] = Some(RoutedEdge { start, segments: vec![straight_cubic(start, end)] });
        }
    }

    let max_x = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.x + g.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x + n.w, n.ax + n.aw]))
        .chain(label_boxes.iter().map(|l| l.0 + l.2))
        .fold(0.0_f64, f64::max);
    let max_y = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.y + g.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y + n.h, n.ay + n.ah]))
        .chain(label_boxes.iter().map(|l| l.1 + l.3))
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
        // Obstacle-routed detour: explicit start anchor + cubic path. The
        // painter draws this instead of a straight center-to-center line.
        if let Some(re) = &routed[ti] {
            write!(out, ", start: ({:.2}pt, {:.2}pt)", re.start.x, re.start.y).unwrap();
            out.push_str(", path: (");
            for seg in &re.segments {
                write!(
                    out,
                    "(c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt), end: ({:.2}pt, {:.2}pt)), ",
                    seg.0.x, seg.0.y, seg.1.x, seg.1.y, seg.2.x, seg.2.y
                )
                .unwrap();
            }
            out.push(')');
        }
        // Reserved label position from the dot-style label node: the
        // painter draws the label just right of this point instead of
        // computing its own midpoint.
        if let Some(p) = label_pos.get(&ti) {
            write!(out, ", label-pos: ({:.2}pt, {:.2}pt)", p.x, p.y).unwrap();
        }
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
