//! Edge routing primitives for class diagrams.
//!
//! Routing is layered (see `super::emit`):
//!
//! 1. Line of sight — single diagonal cubic.
//! 2. Manhattan Z (`try_manhattan_route`) — down-across-down (TB) /
//!    right-up/down-right (LR) when the diagonal is blocked.
//! 3. Pathplan bezier — multi-obstacle detour.
//! 4. Straight cubic — forced fallback.
//!
//! `pick_edge_sides` chooses which face each endpoint anchors to, and
//! `smart_align_coord` decides when both endpoints can share a single
//! coordinate so the Manhattan Z degenerates to a straight segment.

use crate::layout::geometry::Point;
use crate::layout::pathplan;

use super::geom::{ClassGeom, Side};

/// Headroom required inside the perpendicular-axis intersection
/// before we'll smart-align the anchors there. Below this, alignment
/// would push the anchor too close to a corner — fall back to the
/// default mid-side anchor instead.
pub(super) const SMART_ALIGN_HEADROOM_PT: f64 = 4.0;

/// Pick (from-side, to-side) for an edge.
///
/// Rule of thumb: connect along the layout's primary axis (vertical
/// for TB, horizontal for LR) whenever the boxes are on different
/// ranks — i.e. their extents on that axis don't overlap. Use the
/// perpendicular axis only for sibling-rank pairs whose primary-axis
/// extents *do* overlap (a U-turn over the rank gap would otherwise
/// be ugly).
///
/// `from_bbox` / `to_bbox` are `(top_left, bot_right)`.
pub(super) fn pick_edge_sides(
    from_center: Point,
    to_center: Point,
    from_bbox: (Point, Point),
    to_bbox: (Point, Point),
    is_lr: bool,
) -> (Side, Side) {
    let dx = to_center.x - from_center.x;
    let dy = to_center.y - from_center.y;

    let primary_axis_overlap = if is_lr {
        // LR: primary axis is x. Overlap on x means same rank.
        from_bbox.0.x < to_bbox.1.x && to_bbox.0.x < from_bbox.1.x
    } else {
        // TB: primary axis is y. Overlap on y means same rank.
        from_bbox.0.y < to_bbox.1.y && to_bbox.0.y < from_bbox.1.y
    };

    // Sibling rank (boxes overlap on the rank axis): a primary-axis
    // edge would U-turn, so anchor on the perpendicular axis.
    // Different rank: always anchor along the primary axis so the
    // edge runs with the rank flow (looks like a Sugiyama tree fan-out
    // rather than a horizontal cross-cut). Tangents and obstacle
    // routing handle long-distance fan-outs without crowding.
    let prefer_horizontal = if primary_axis_overlap { true } else { is_lr };
    if prefer_horizontal {
        if dx >= 0.0 {
            (Side::Right, Side::Left)
        } else {
            (Side::Left, Side::Right)
        }
    } else if dy >= 0.0 {
        (Side::Bot, Side::Top)
    } else {
        (Side::Top, Side::Bot)
    }
}

/// Unit tangent pointing *outward* from a box face — used as the
/// launch / arrival tangent for cubic edge routing so the bezier
/// leaves the anchor perpendicular to the face it attaches to.
pub(super) fn side_tangent(side: Side) -> Point {
    match side {
        Side::Top => Point::new(0.0, -1.0),
        Side::Bot => Point::new(0.0, 1.0),
        Side::Left => Point::new(-1.0, 0.0),
        Side::Right => Point::new(1.0, 0.0),
    }
}

/// If both anchors are on the same axis (both left/right OR both
/// top/bot) and the boxes' perpendicular extents *and* both boxes'
/// centers fit inside that overlap, return the coordinate at which
/// both anchors should be placed so the Manhattan Z degenerates to a
/// single straight segment.
///
/// The "both centers in overlap" gate is what makes fan-in / fan-out
/// (e.g. basic.puml's Dog and Cat both pointing at Animal) keep
/// their natural anchors. Without it, smart-align would yank a
/// source's anchor far from its own mid, making the edge appear to
/// leave a corner of the parent box rather than the centre — the
/// look that makes fan-outs read as "lines hooked to nowhere".
pub(super) fn smart_align_coord(
    from_g: &ClassGeom,
    from_tl: Point,
    to_g: &ClassGeom,
    to_tl: Point,
    from_side: Side,
    to_side: Side,
) -> Option<f64> {
    let from_horizontal = matches!(from_side, Side::Left | Side::Right);
    let to_horizontal = matches!(to_side, Side::Left | Side::Right);
    if from_horizontal != to_horizontal {
        return None;
    }
    if from_horizontal {
        // Both anchors are on left/right side; align on y.
        let lo = from_tl.y.max(to_tl.y);
        let hi = (from_tl.y + from_g.size.y).min(to_tl.y + to_g.size.y);
        if hi - lo <= SMART_ALIGN_HEADROOM_PT {
            return None;
        }
        let from_mid = from_tl.y + from_g.size.y / 2.0;
        let to_mid = to_tl.y + to_g.size.y / 2.0;
        if !(lo <= from_mid && from_mid <= hi && lo <= to_mid && to_mid <= hi) {
            return None;
        }
        Some((from_mid + to_mid) / 2.0)
    } else {
        // Both anchors are on top/bot; align on x.
        let lo = from_tl.x.max(to_tl.x);
        let hi = (from_tl.x + from_g.size.x).min(to_tl.x + to_g.size.x);
        if hi - lo <= SMART_ALIGN_HEADROOM_PT {
            return None;
        }
        let from_mid = from_tl.x + from_g.mid_x;
        let to_mid = to_tl.x + to_g.mid_x;
        if !(lo <= from_mid && from_mid <= hi && lo <= to_mid && to_mid <= hi) {
            return None;
        }
        // Average of mids — anchors land close to *both* boxes' centres,
        // so neither edge appears off-centre.
        Some((from_mid + to_mid) / 2.0)
    }
}

/// Try to route `start → end` as 3 axis-aligned segments. For TB
/// (vertical = true), a "down → across → down" Z; for LR (vertical =
/// false), a "right → up/down → right" Z. Returns `None` if any
/// segment would clip a class bbox in `obstacles`.
pub(super) fn try_manhattan_route(
    start: Point,
    end: Point,
    obstacles: &[pathplan::Box],
    vertical: bool,
) -> Option<Vec<(Point, Point, Point)>> {
    const TOL: f64 = 1.0;
    let parallel = if vertical {
        (start.x - end.x).abs() < TOL
    } else {
        (start.y - end.y).abs() < TOL
    };
    if parallel {
        // Source and target share the cross-axis coord — single straight
        // cubic.
        let c1 = Point::new(
            start.x + (end.x - start.x) / 3.0,
            start.y + (end.y - start.y) / 3.0,
        );
        let c2 = Point::new(
            start.x + 2.0 * (end.x - start.x) / 3.0,
            start.y + 2.0 * (end.y - start.y) / 3.0,
        );
        if obstacles
            .iter()
            .any(|ob| seg_intersects_box(start, end, ob))
        {
            return None;
        }
        return Some(vec![(c1, c2, end)]);
    }

    // Z-route. For vertical: turn at mid-y. For horizontal: turn at
    // mid-x. If the mid bend clips an obstacle, retry with bends
    // placed just outside each blocking obstacle — this is the
    // detour-around-sibling-cluster case introduced by M4 (the cluster
    // bbox is now part of `obstacles`, so the original mid bend often
    // passes through the cluster's interior).
    let p1 = start;
    let p4 = end;

    let mid = if vertical {
        (start.y + end.y) / 2.0
    } else {
        (start.x + end.x) / 2.0
    };

    if let Some(segs) = try_z_with_bend(p1, p4, mid, vertical, obstacles) {
        return Some(segs);
    }

    // mid bend blocked — enumerate fallback bend coordinates just
    // outside every obstacle's blocking axis. Padding nudges the bend
    // off the obstacle face so the segment runs along the outside
    // rather than tangent to the wall.
    const BEND_PAD: f64 = 8.0;
    let mut candidates: Vec<f64> = Vec::new();
    for ob in obstacles {
        if vertical {
            candidates.push(ob.min.y - BEND_PAD);
            candidates.push(ob.max.y + BEND_PAD);
        } else {
            candidates.push(ob.min.x - BEND_PAD);
            candidates.push(ob.max.x + BEND_PAD);
        }
    }
    // Sort by closeness to mid so we prefer least-deviating routes.
    candidates.sort_by(|a, b| (a - mid).abs().partial_cmp(&(b - mid).abs()).unwrap());
    for c in candidates {
        if let Some(segs) = try_z_with_bend(p1, p4, c, vertical, obstacles) {
            return Some(segs);
        }
    }

    None
}

/// Build a 3-segment Z route with the bend on the cross axis at
/// coordinate `bend`. `vertical = true` means the long edge runs
/// vertically (bend is a y-coordinate, segments are vert/horiz/vert);
/// `vertical = false` flips the axes. Returns `None` if any segment
/// clips an obstacle.
fn try_z_with_bend(
    p1: Point,
    p4: Point,
    bend: f64,
    vertical: bool,
    obstacles: &[pathplan::Box],
) -> Option<Vec<(Point, Point, Point)>> {
    let (p2, p3) = if vertical {
        (Point::new(p1.x, bend), Point::new(p4.x, bend))
    } else {
        (Point::new(bend, p1.y), Point::new(bend, p4.y))
    };
    for ob in obstacles {
        if seg_intersects_box(p1, p2, ob)
            || seg_intersects_box(p2, p3, ob)
            || seg_intersects_box(p3, p4, ob)
        {
            return None;
        }
    }
    Some(vec![
        cubic_from_straight(p1, p2),
        cubic_from_straight(p2, p3),
        cubic_from_straight(p3, p4),
    ])
}

/// True iff a straight line from `a` to `b` doesn't cross any
/// obstacle bbox (open boundary — touching a corner is allowed).
/// Used to decide whether an edge can take a single diagonal cubic
/// instead of detouring through Manhattan or pathplan.
pub(super) fn line_of_sight_clear(a: Point, b: Point, obstacles: &[pathplan::Box]) -> bool {
    obstacles
        .iter()
        .all(|ob| !segment_strictly_crosses_box(a, b, ob))
}

/// True iff segment a→b enters the open interior of `ob`. Endpoints
/// touching the box border count as outside (so an edge that anchors
/// on a face doesn't read as crossing its own anchor box).
fn segment_strictly_crosses_box(a: Point, b: Point, ob: &pathplan::Box) -> bool {
    let lo = ob.min;
    let hi = ob.max;
    // Liang-Barsky-ish parameter clipping. Compute the t-interval over
    // which the parametric segment lies inside [lo, hi]^2; reject if
    // empty or only at the endpoints.
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_enter = 0.0_f64;
    let mut t_exit = 1.0_f64;
    let clip = |p: f64, q: f64, t_enter: &mut f64, t_exit: &mut f64| -> bool {
        if p.abs() < 1e-9 {
            // Parallel to slab: inside iff q >= 0.
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > *t_exit { return false; }
            if r > *t_enter { *t_enter = r; }
        } else {
            if r < *t_enter { return false; }
            if r < *t_exit { *t_exit = r; }
        }
        true
    };
    if !clip(-dx, a.x - lo.x, &mut t_enter, &mut t_exit) { return false; }
    if !clip(dx, hi.x - a.x, &mut t_enter, &mut t_exit) { return false; }
    if !clip(-dy, a.y - lo.y, &mut t_enter, &mut t_exit) { return false; }
    if !clip(dy, hi.y - a.y, &mut t_enter, &mut t_exit) { return false; }
    // Strict interior: we need the clipped sub-segment to have
    // positive length (more than just touching at a single t).
    t_exit - t_enter > 1e-6
}

/// Build a smooth path from `a` to `b` whose tangent at both ends
/// follows `src_normal` (the outward face normal). Returns one or
/// two cubic segments:
///
/// * Single cubic for short hops where a clean S-curve is enough.
/// * Two cubics for longer hops: a sweeping cubic from `a` to a
///   pre-arrival point that's `HEAD_STUB_PT` along the normal away
///   from `b`, then a short straight cubic from there to `b`. The
///   stub guarantees the line is genuinely axis-aligned for the last
///   stretch — without it, a single cubic with perpendicular
///   tangents at both ends would still cross the arrowhead's base
///   off-centre, because the cubic's *trajectory* near the endpoint
///   doesn't go straight even when its tangent does.
pub(super) fn diagonal_path(
    a: Point,
    b: Point,
    src_normal: Point,
) -> Vec<(Point, Point, Point)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let along = (dx * src_normal.x + dy * src_normal.y).abs();
    let perp = ((dx * -src_normal.y) + (dy * src_normal.x)).abs();
    // Reserve room for the arrow head plus a 2pt safety margin so the
    // base of a triangle/diamond head sits inside the axis-aligned tail.
    const HEAD_STUB_PT: f64 = 12.0;
    // No room to insert a stub — fall back to a single S-cubic.
    if along <= HEAD_STUB_PT * 1.5 || perp < 1.0 {
        let handle = (along * 0.5).max(8.0).min(40.0);
        let c1 = Point::new(a.x + src_normal.x * handle, a.y + src_normal.y * handle);
        let c2 = Point::new(b.x - src_normal.x * handle, b.y - src_normal.y * handle);
        return vec![(c1, c2, b)];
    }
    // Pre-arrival point: HEAD_STUB_PT along the *outward* normal back
    // from b. The sweep ends here; the stub from here to b is axis
    // aligned by construction.
    let pre = Point::new(b.x - src_normal.x * HEAD_STUB_PT, b.y - src_normal.y * HEAD_STUB_PT);
    let along_to_pre = along - HEAD_STUB_PT;
    let handle = (along_to_pre * 0.5).max(8.0).min(40.0);
    let c1 = Point::new(a.x + src_normal.x * handle, a.y + src_normal.y * handle);
    let c2 = Point::new(pre.x - src_normal.x * handle, pre.y - src_normal.y * handle);
    let sweep = (c1, c2, pre);
    let stub = cubic_from_straight(pre, b);
    vec![sweep, stub]
}

/// Express a straight line a→b as a (c1, c2, end) cubic Bezier whose
/// path is exactly the line. Control handles sit at 1/3 and 2/3 along.
pub(super) fn cubic_from_straight(a: Point, b: Point) -> (Point, Point, Point) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (
        Point::new(a.x + dx / 3.0, a.y + dy / 3.0),
        Point::new(a.x + 2.0 * dx / 3.0, a.y + 2.0 * dy / 3.0),
        b,
    )
}

/// True iff the axis-aligned segment a→b touches the rectangle `ob`.
/// We only call this with axis-aligned (vertical or horizontal)
/// segments — the diagonal branch returns `false` defensively.
fn seg_intersects_box(a: Point, b: Point, ob: &pathplan::Box) -> bool {
    let lo = ob.min;
    let hi = ob.max;
    if (a.x - b.x).abs() < 1e-6 {
        // Vertical segment at x.
        let x = a.x;
        if x <= lo.x || x >= hi.x {
            return false;
        }
        let y_lo = a.y.min(b.y);
        let y_hi = a.y.max(b.y);
        return !(y_hi <= lo.y || y_lo >= hi.y);
    }
    if (a.y - b.y).abs() < 1e-6 {
        // Horizontal segment at y.
        let y = a.y;
        if y <= lo.y || y >= hi.y {
            return false;
        }
        let x_lo = a.x.min(b.x);
        let x_hi = a.x.max(b.x);
        return !(x_hi <= lo.x || x_lo >= hi.x);
    }
    false
}

pub(super) fn straight_fallback(
    start: Point,
    end: Point,
    force_max: f64,
) -> Vec<(Point, Point, Point)> {
    let dist = start.distance_to(end);
    let force = (dist * 0.4).min(force_max);
    let c1 = start.add(Point::new(0.0, force));
    let c2 = end.sub(Point::new(0.0, force));
    vec![(c1, c2, end)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(w: f64, h: f64) -> ClassGeom {
        ClassGeom {
            size: Point::new(w, h),
            mid_x: w / 2.0,
        }
    }

    #[test]
    fn pick_sides_tb_different_rank_uses_vertical_anchors() {
        // TB, source above target — anchor bot → top.
        let from_c = Point::new(50.0, 25.0);
        let to_c = Point::new(50.0, 125.0);
        let from_bb = (Point::new(0.0, 0.0), Point::new(100.0, 50.0));
        let to_bb = (Point::new(0.0, 100.0), Point::new(100.0, 150.0));
        let (fs, ts) = pick_edge_sides(from_c, to_c, from_bb, to_bb, false);
        assert_eq!(fs, Side::Bot);
        assert_eq!(ts, Side::Top);
    }

    #[test]
    fn pick_sides_tb_sibling_rank_uses_horizontal_anchors() {
        // Same y range — anchor right → left so the edge doesn't U-turn.
        let from_c = Point::new(50.0, 50.0);
        let to_c = Point::new(250.0, 50.0);
        let from_bb = (Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let to_bb = (Point::new(200.0, 0.0), Point::new(300.0, 100.0));
        let (fs, ts) = pick_edge_sides(from_c, to_c, from_bb, to_bb, false);
        assert_eq!(fs, Side::Right);
        assert_eq!(ts, Side::Left);
    }

    #[test]
    fn pick_sides_lr_different_rank_uses_horizontal_anchors() {
        // LR mode, primary axis is x. Boxes don't overlap on x → use
        // primary-axis anchors (right → left).
        let from_c = Point::new(50.0, 50.0);
        let to_c = Point::new(250.0, 50.0);
        let from_bb = (Point::new(0.0, 0.0), Point::new(100.0, 100.0));
        let to_bb = (Point::new(200.0, 0.0), Point::new(300.0, 100.0));
        let (fs, ts) = pick_edge_sides(from_c, to_c, from_bb, to_bb, true);
        assert_eq!(fs, Side::Right);
        assert_eq!(ts, Side::Left);
    }

    #[test]
    fn smart_align_coord_returns_none_when_sides_mismatch() {
        let a = g(40.0, 30.0);
        let b = g(40.0, 30.0);
        // One anchor on Top, other on Right — different axes.
        assert!(smart_align_coord(
            &a,
            Point::new(0.0, 0.0),
            &b,
            Point::new(100.0, 100.0),
            Side::Top,
            Side::Right,
        )
        .is_none());
    }

    #[test]
    fn smart_align_coord_returns_some_when_both_centers_in_overlap() {
        // Two boxes with the same x-range, vertically stacked. Both
        // sides Bot/Top — should smart-align to the shared mid-x.
        let a = g(40.0, 30.0);
        let b = g(40.0, 30.0);
        let coord = smart_align_coord(
            &a,
            Point::new(10.0, 0.0),
            &b,
            Point::new(10.0, 100.0),
            Side::Bot,
            Side::Top,
        )
        .expect("aligned");
        // Mid-x of both boxes = 10 + 20 = 30. Average of mids = 30.
        assert!((coord - 30.0).abs() < 1e-6);
    }

    #[test]
    fn smart_align_coord_none_when_centers_outside_overlap() {
        // Boxes overlap on x only at edges; one center sits outside
        // the overlap → reject (would push anchor near a corner).
        let a = g(40.0, 30.0);
        let b = g(40.0, 30.0);
        // a x-range [0, 40], b x-range [30, 70]. Overlap [30, 40].
        // a.mid_x = 20 (outside overlap), b.mid_x = 50 (also outside).
        assert!(smart_align_coord(
            &a,
            Point::new(0.0, 0.0),
            &b,
            Point::new(30.0, 100.0),
            Side::Bot,
            Side::Top,
        )
        .is_none());
    }

    #[test]
    fn smart_align_coord_none_when_headroom_too_small() {
        // Boxes barely overlap perpendicular (less than the headroom).
        let a = g(40.0, 30.0);
        let b = g(40.0, 30.0);
        // a.x-range [0, 40], b.x-range [38, 78]. Overlap [38, 40] = 2pt
        // which is below the 4pt headroom.
        assert!(smart_align_coord(
            &a,
            Point::new(0.0, 0.0),
            &b,
            Point::new(38.0, 100.0),
            Side::Bot,
            Side::Top,
        )
        .is_none());
    }

    #[test]
    fn try_manhattan_route_parallel_yields_single_cubic() {
        // Source and target share x → single straight cubic on the
        // vertical line. No obstacles.
        let segs = try_manhattan_route(
            Point::new(50.0, 0.0),
            Point::new(50.0, 100.0),
            &[],
            true,
        )
        .expect("clear");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn try_manhattan_route_z_yields_three_segments() {
        let segs = try_manhattan_route(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            &[],
            true,
        )
        .expect("clear");
        assert_eq!(segs.len(), 3);
    }

    #[test]
    fn try_manhattan_route_blocked_falls_back_to_detour_bend() {
        // Mid-bend Z (mid-y = 50) would clip this obstacle (40-60).
        // Improved router falls back to a bend just outside the
        // obstacle's top or bottom edge instead of giving up.
        let ob = pathplan::Box::new(Point::new(20.0, 40.0), Point::new(80.0, 60.0));
        let segs = try_manhattan_route(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            &[ob],
            true,
        )
        .expect("must find a detour bend (above or below the obstacle)");
        assert_eq!(segs.len(), 3);
        // The middle segment runs horizontally at the bend y — it must
        // be above or below the obstacle, not through it.
        let bend_y = segs[1].2.y; // end of segment 2 = bend point
        assert!(
            bend_y < 40.0 || bend_y > 60.0,
            "detour bend y={bend_y} should be outside obstacle [40, 60]"
        );
    }

    #[test]
    fn try_manhattan_route_truly_unreachable_returns_none() {
        // Two stacked obstacles that completely cover every horizontal
        // band a Z bend could occupy: no horizontal line from x=0 to
        // x=100 is clear, so even the detour-bend fallback can't find
        // a route.
        let ob_a = pathplan::Box::new(Point::new(-100.0, -100.0), Point::new(200.0, 50.0));
        let ob_b = pathplan::Box::new(Point::new(-100.0, 50.0), Point::new(200.0, 200.0));
        let segs = try_manhattan_route(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            &[ob_a, ob_b],
            true,
        );
        // Note: start/end are INSIDE the obstacles in this contrived
        // test — every candidate bend will still clip one of them.
        // The function returns None and the caller falls back to
        // pathplan (or the straight-cubic last resort).
        assert!(segs.is_none(), "fully-blocked Z must return None");
    }

    #[test]
    fn line_of_sight_clear_detects_strict_crossing() {
        let ob = pathplan::Box::new(Point::new(40.0, 40.0), Point::new(60.0, 60.0));
        // Diagonal that cuts the box → not clear.
        assert!(!line_of_sight_clear(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            &[ob],
        ));
        // Touching only a corner still counts as clear (open-boundary
        // rule).
        assert!(line_of_sight_clear(
            Point::new(0.0, 0.0),
            Point::new(40.0, 40.0),
            &[ob],
        ));
    }
}
