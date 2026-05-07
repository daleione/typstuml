//! Obstacle-aware polyline routing for record-graph edges.
//!
//! Given source/target endpoints and a list of axis-aligned rectangles to
//! avoid, returns a polyline from source to target that doesn't pass
//! through any rectangle's interior. Used by `record_graph.rs` to route
//! edges around unrelated records — without this, edges like
//! `phones[1] → work` on `docs/t.puml` cut straight through `addresses[0]`.
//!
//! Algorithm: visibility graph + Dijkstra. Candidate waypoints are the
//! source, the target, and the four corners of each (padded) obstacle.
//! Two waypoints are connected iff the segment between them doesn't
//! enter any obstacle's strict interior. Shortest path is the route.
//!
//! For record graphs n is small (≤ 30 records), so the O(n²) visibility
//! construction is fine. The output polyline has 2 points when the
//! straight line is unobstructed (so simple diagrams render exactly as
//! before), and 3+ points otherwise.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::layout::geometry::{segment_rect_intersection, Point};

/// Find a polyline from `start` to `end` that avoids the interior of every
/// obstacle. `obstacles` are axis-aligned rectangles `(top_left,
/// bottom_right)`. `padding` is added around each obstacle so the path
/// stays a small margin away from record edges.
///
/// Always returns at least `[start, end]`. When the straight segment is
/// clear, returns exactly that. When it's blocked, returns the shortest
/// detour through the obstacle-corner visibility graph; the detour
/// includes `start` and `end` plus 1+ intermediate waypoints.
pub fn find_polyline(
    start: Point,
    end: Point,
    obstacles: &[(Point, Point)],
    padding: f64,
) -> Vec<Point> {
    let padded: Vec<(Point, Point)> = obstacles
        .iter()
        .filter(|r| !endpoint_inside_padded(start, end, **r, padding))
        .map(|r| pad_rect(*r, padding))
        .collect();

    if !blocked_by_any(start, end, &padded) {
        return vec![start, end];
    }

    let mut waypoints = Vec::with_capacity(2 + 4 * padded.len());
    waypoints.push(start);
    waypoints.push(end);
    for (tl, br) in &padded {
        waypoints.push(*tl);
        waypoints.push(Point::new(br.x, tl.y));
        waypoints.push(Point::new(tl.x, br.y));
        waypoints.push(*br);
    }

    let n = waypoints.len();
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            if !blocked_by_any(waypoints[i], waypoints[j], &padded) {
                let d = waypoints[i].distance_to(waypoints[j]);
                adj[i].push((j, d));
                adj[j].push((i, d));
            }
        }
    }

    match dijkstra(&adj, 0, 1) {
        Some(idx_path) => idx_path.into_iter().map(|i| waypoints[i]).collect(),
        None => vec![start, end],
    }
}

/// Compute cubic-bezier segments through a polyline. Each tuple is
/// `(c1, c2, end)` for one segment; the first segment starts at
/// `polyline[0]`, every subsequent segment starts where the previous
/// segment ended. Tangents at the start, end, and intermediate
/// waypoints control the smoothness:
///
/// - start / end tangent: horizontal (axis along which records align),
///   so the curve leaves and enters records cleanly through their
///   left/right edges.
/// - waypoint tangent: bisector of incoming and outgoing segment
///   directions, so the corner rounds smoothly.
///
/// `force_max` caps the control-handle length so short segments don't
/// overshoot.
pub fn compute_bezier_segments(
    polyline: &[Point],
    force_max: f64,
) -> Vec<(Point, Point, Point)> {
    let n = polyline.len();
    if n < 2 {
        return Vec::new();
    }
    let tangents: Vec<Point> = (0..n).map(|i| tangent_at(polyline, i)).collect();

    let mut out = Vec::with_capacity(n - 1);
    for i in 0..(n - 1) {
        let dist = polyline[i].distance_to(polyline[i + 1]);
        let force = (dist * 0.4).min(force_max);
        let c1 = polyline[i].add(tangents[i].scale(force));
        let c2 = polyline[i + 1].sub(tangents[i + 1].scale(force));
        out.push((c1, c2, polyline[i + 1]));
    }
    out
}

fn tangent_at(polyline: &[Point], i: usize) -> Point {
    let n = polyline.len();
    if i == 0 || i == n - 1 {
        // Endpoints leave / enter records along the rank axis (horizontal
        // for LR record graphs). A constant horizontal tangent is the
        // closest analogue to dot's record-edge handling.
        return Point::new(1.0, 0.0);
    }
    let prev = unit_or(polyline[i].sub(polyline[i - 1]), Point::new(1.0, 0.0));
    let next = unit_or(polyline[i + 1].sub(polyline[i]), Point::new(1.0, 0.0));
    let avg = prev.add(next);
    let len = avg.length();
    if len < 1e-9 {
        // Anti-parallel; rotate 90° to break the tie.
        Point::new(-prev.y, prev.x)
    } else {
        avg.scale(1.0 / len)
    }
}

fn unit_or(v: Point, fallback: Point) -> Point {
    let len = v.length();
    if len < 1e-9 {
        fallback
    } else {
        v.scale(1.0 / len)
    }
}

fn pad_rect(rect: (Point, Point), pad: f64) -> (Point, Point) {
    (
        Point::new(rect.0.x - pad, rect.0.y - pad),
        Point::new(rect.1.x + pad, rect.1.y + pad),
    )
}

/// Skip an obstacle when an endpoint sits inside the padded rectangle —
/// the edge has to pass through that point regardless, so treating it
/// as a blocker would only spuriously kill the visibility graph.
fn endpoint_inside_padded(
    start: Point,
    end: Point,
    rect: (Point, Point),
    pad: f64,
) -> bool {
    let padded = pad_rect(rect, pad);
    point_inside(start, padded) || point_inside(end, padded)
}

fn point_inside(p: Point, rect: (Point, Point)) -> bool {
    p.x > rect.0.x && p.x < rect.1.x && p.y > rect.0.y && p.y < rect.1.y
}

fn blocked_by_any(a: Point, b: Point, padded: &[(Point, Point)]) -> bool {
    padded
        .iter()
        .any(|r| segment_intersects_interior(a, b, *r))
}

/// True iff the segment crosses the rectangle's strict interior. A
/// segment that grazes only the perimeter (endpoint on edge) is
/// treated as not-blocking, so visibility-graph candidates that touch
/// obstacle corners stay reachable.
fn segment_intersects_interior(a: Point, b: Point, rect: (Point, Point)) -> bool {
    let eps = 0.05;
    let shrunken = (
        Point::new(rect.0.x + eps, rect.0.y + eps),
        Point::new(rect.1.x - eps, rect.1.y - eps),
    );
    if shrunken.0.x >= shrunken.1.x || shrunken.0.y >= shrunken.1.y {
        return false;
    }
    segment_rect_intersection((a, b), shrunken)
}

fn dijkstra(adj: &[Vec<(usize, f64)>], src: usize, dst: usize) -> Option<Vec<usize>> {
    let n = adj.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    dist[src] = 0.0;
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::new();
    heap.push(HeapEntry(0.0, src));

    while let Some(HeapEntry(d, u)) = heap.pop() {
        if u == dst {
            break;
        }
        if d > dist[u] {
            continue;
        }
        for &(v, w) in &adj[u] {
            let nd = d + w;
            if nd < dist[v] {
                dist[v] = nd;
                prev[v] = u;
                heap.push(HeapEntry(nd, v));
            }
        }
    }

    if dist[dst].is_infinite() {
        return None;
    }
    let mut path = Vec::new();
    let mut cur = dst;
    while cur != src {
        path.push(cur);
        cur = prev[cur];
        if cur == usize::MAX {
            return None;
        }
    }
    path.push(src);
    path.reverse();
    Some(path)
}

#[derive(Copy, Clone)]
struct HeapEntry(f64, usize);

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Reverse order so BinaryHeap behaves as a min-heap on `dist`.
        other.0.partial_cmp(&self.0)
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn straight_line_when_no_obstacles() {
        let path = find_polyline(pt(0.0, 0.0), pt(100.0, 0.0), &[], 2.0);
        assert_eq!(path, vec![pt(0.0, 0.0), pt(100.0, 0.0)]);
    }

    #[test]
    fn straight_line_when_obstacle_offset() {
        // Obstacle below the path; straight line stays clear.
        let obstacles = [(pt(20.0, 50.0), pt(80.0, 100.0))];
        let path = find_polyline(pt(0.0, 0.0), pt(100.0, 0.0), &obstacles, 2.0);
        assert_eq!(path, vec![pt(0.0, 0.0), pt(100.0, 0.0)]);
    }

    #[test]
    fn detours_around_blocking_obstacle() {
        // Obstacle right on the straight line. Path must detour above or
        // below, picking up at least one intermediate waypoint.
        let obstacles = [(pt(40.0, -10.0), pt(60.0, 10.0))];
        let path = find_polyline(pt(0.0, 0.0), pt(100.0, 0.0), &obstacles, 2.0);
        assert!(path.len() >= 3, "expected detour, got {:?}", path);
        assert_eq!(path[0], pt(0.0, 0.0));
        assert_eq!(*path.last().unwrap(), pt(100.0, 0.0));
        // The detour should clear the obstacle on the perpendicular axis.
        let clears = path
            .iter()
            .any(|p| p.y > 12.0 || p.y < -12.0 || p.x < 38.0 || p.x > 62.0);
        assert!(clears, "no waypoint clears the obstacle: {:?}", path);
    }

    #[test]
    fn picks_shorter_side() {
        // Asymmetric obstacle: top edge much closer to the straight line
        // than the bottom. Detour should go over the top.
        let obstacles = [(pt(40.0, -3.0), pt(60.0, 100.0))];
        let path = find_polyline(pt(0.0, 0.0), pt(100.0, 0.0), &obstacles, 2.0);
        let took_top = path.iter().all(|p| p.y < 50.0);
        assert!(took_top, "expected top detour, got {:?}", path);
    }

    #[test]
    fn segments_horizontal_at_endpoints() {
        // Single-segment polyline → c1, c2 stay on the start/end y.
        let segs = compute_bezier_segments(&[pt(0.0, 0.0), pt(100.0, 0.0)], 30.0);
        assert_eq!(segs.len(), 1);
        let (c1, c2, end) = segs[0];
        assert!((c1.y).abs() < 1e-9);
        assert!((c2.y).abs() < 1e-9);
        assert_eq!(end, pt(100.0, 0.0));
    }

    #[test]
    fn segments_smooth_at_corner() {
        // Two-segment polyline with a 90° turn. Tangent at the corner is
        // the bisector, so c2 of segment 0 and c1 of segment 1 are
        // anti-parallel about the corner waypoint.
        let segs = compute_bezier_segments(
            &[pt(0.0, 0.0), pt(50.0, 0.0), pt(50.0, 50.0)],
            30.0,
        );
        assert_eq!(segs.len(), 2);
        // c2 of segment 0 sits on the bisector (45°) from waypoint (50,0).
        let (_, c2, _) = segs[0];
        let off = c2.sub(pt(50.0, 0.0));
        // Bisector tangent normalised is (1/√2, 1/√2); c2 = waypoint - tangent*force,
        // so off.x and off.y should be equal in magnitude, both negative.
        assert!(off.x < 0.0 && off.y < 0.0, "got {:?}", off);
        assert!((off.x - off.y).abs() < 1e-6, "not on bisector: {:?}", off);
    }
}
