//! Geometric primitives and the obstacle-aware transition router.
//!
//! Node-perimeter clipping, segment/box intersection, and the spline router
//! that detours obstructed transitions around composite frames and sibling
//! leaf boxes.

use crate::ir::{StateDiagram, StateKind};
use crate::layout::geometry::Point;

use super::view::{NodeBoxes, NodeTopology};

/// Perimeter-shape class for a node kind — mirrors `states.typ::_shape-of`.
pub(super) fn node_shape(kind: StateKind) -> &'static str {
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
pub(super) fn perimeter_point(c: Point, hw: f64, hh: f64, shape: &str, toward: Point) -> Point {
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
pub(super) fn seg_crosses_box(a: Point, b: Point, lo: Point, hi: Point) -> bool {
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
pub(super) struct RoutedEdge {
    pub(super) start: Point,
    pub(super) segments: Vec<(Point, Point, Point)>,
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
pub(super) fn route_transitions(
    diag: &StateDiagram,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    is_lr: bool,
) -> Vec<Option<RoutedEdge>> {
    let n = diag.nodes.len();
    let topo = NodeTopology::new(diag);
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);

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
        let (s, d) = match (topo.index(&tr.from), topo.index(&tr.to)) {
            (Some(s), Some(d)) => (s, d),
            _ => continue,
        };
        // `n` is involved with this edge (so never an obstacle) when it
        // contains or is contained by either endpoint.
        let involved = |x: usize| {
            topo.anc_or_self(x, s)
                || topo.anc_or_self(x, d)
                || topo.anc_or_self(s, x)
                || topo.anc_or_self(d, x)
        };
        let is_blocker = |x: usize| !involved(x);
        let start = boxes.perimeter_toward(s, boxes.center(d));
        let end = boxes.perimeter_toward(d, boxes.center(s));
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
            if let Some(p) = topo.parent(x) {
                if is_blocker(p) {
                    continue;
                }
            }
            let (lo, hi) = boxes.bbox(x);
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
        pending.push(Pending {
            ti,
            start,
            end,
            u_lo,
            u_hi,
        });
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
        out[p.ti] = Some(RoutedEdge {
            start: p.start,
            segments,
        });
    }
    out
}

/// Smooth a polyline `pts` (≥2 points) into a chain of cubic bezier
/// segments passing through every point, using the Catmull-Rom →
/// Bezier construction (tangent at each interior point is parallel to the
/// chord between its neighbours, scaled by 1/6). A 2-point polyline yields
/// a straight cubic; a polyline that bends (a long edge running out to a
/// side lane and back) yields a smooth arc — dot's spline look without the
/// full pathplan router.
pub(super) fn smooth_polyline(pts: &[Point]) -> Vec<(Point, Point, Point)> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let mut segs = Vec::with_capacity(pts.len() - 1);
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = Point::new(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0);
        let c2 = Point::new(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0);
        segs.push((c1, c2, p2));
    }
    segs
}

/// A straight line `a→b` expressed as a single cubic whose control
/// handles sit at 1/3 and 2/3 — the painter draws it as the segment.
pub(super) fn straight_cubic(a: Point, b: Point) -> (Point, Point, Point) {
    let c1 = Point::new(a.x + (b.x - a.x) / 3.0, a.y + (b.y - a.y) / 3.0);
    let c2 = Point::new(a.x + 2.0 * (b.x - a.x) / 3.0, a.y + 2.0 * (b.y - a.y) / 3.0);
    (c1, c2, b)
}

/// Build an axis-aligned detour from `start` to `end` whose long run sits
/// at `side_coord` on the perpendicular axis (an x in TB, a y in LR).
/// Returns three cubic segments with sharp orthogonal corners — clean and
/// unambiguous, matching PlantUML's `splines=ortho` look for routed
/// cross-edges.
pub(super) fn detour_around(
    start: Point,
    end: Point,
    side_coord: f64,
    is_lr: bool,
) -> Vec<(Point, Point, Point)> {
    let (p1, p2) = if is_lr {
        (
            Point::new(start.x, side_coord),
            Point::new(end.x, side_coord),
        )
    } else {
        (
            Point::new(side_coord, start.y),
            Point::new(side_coord, end.y),
        )
    };
    vec![
        straight_cubic(start, p1),
        straight_cubic(p1, p2),
        straight_cubic(p2, end),
    ]
}
