//! Convert an orthogonal polyline into painter-ready cubic segments:
//! straight runs stay straight, and each interior corner is rounded
//! with a quarter-circle-ish cubic (kappa 0.5523), clamped so the
//! rounding never eats more than half of either adjacent segment.

use crate::layout::geometry::Point;

/// Bezier circle-arc approximation constant (4/3 * tan(pi/8)).
const KAPPA: f64 = 0.5522847498;

/// Remove points that don't change direction (collinear runs) so
/// rounding only happens at genuine turns. `tol` is the perpendicular
/// distance tolerance for "still collinear".
pub fn simplify(points: &[Point], tol: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        let a = *out.last().unwrap();
        let b = points[i];
        let c = points[i + 1];
        if !collinear(a, b, c, tol) {
            out.push(b);
        }
    }
    out.push(*points.last().unwrap());
    out
}

fn collinear(a: Point, b: Point, c: Point, tol: f64) -> bool {
    // Only ever called on axis-aligned polylines, so "collinear" means
    // either both segments are vertical at the same x, or both
    // horizontal at the same y.
    let ab_vertical = (a.x - b.x).abs() < tol;
    let bc_vertical = (b.x - c.x).abs() < tol;
    let ab_horizontal = (a.y - b.y).abs() < tol;
    let bc_horizontal = (b.y - c.y).abs() < tol;
    (ab_vertical && bc_vertical && (a.x - c.x).abs() < tol)
        || (ab_horizontal && bc_horizontal && (a.y - c.y).abs() < tol)
}

/// Turn a simplified orthogonal polyline into a list of `(c1, c2,
/// end)` cubics — the exact shape the blockcell `_draw-edge` painter
/// already consumes for spline routes. Straight runs become a
/// straight-line cubic (control points at 1/3 and 2/3); each interior
/// bend becomes a rounded corner of radius `arc` (clamped to half the
/// shorter of its two adjacent segments so tight corners don't
/// overshoot).
pub fn to_rounded_cubics(points: &[Point], arc: f64) -> Vec<(Point, Point, Point)> {
    if points.len() < 2 {
        return Vec::new();
    }
    if points.len() == 2 {
        return vec![straight_cubic(points[0], points[1])];
    }

    let mut out = Vec::with_capacity((points.len() - 1) * 2);
    // Effective corner radius per interior vertex, clamped to half the
    // shorter adjacent segment so two close bends never overlap.
    let mut radii = vec![0.0; points.len()];
    for i in 1..points.len() - 1 {
        let seg_in = points[i].distance_to(points[i - 1]);
        let seg_out = points[i].distance_to(points[i + 1]);
        radii[i] = arc.min(seg_in / 2.0).min(seg_out / 2.0).max(0.0);
    }

    let mut cursor = points[0];
    for i in 1..points.len() - 1 {
        let prev = points[i - 1];
        let corner = points[i];
        let next = points[i + 1];
        let r = radii[i];
        if r < 1e-6 {
            // No room to round — hard corner (rare: back-to-back tiny
            // segments). Emit a straight cubic into the corner; the
            // next iteration continues from it.
            out.push(straight_cubic(cursor, corner));
            cursor = corner;
            continue;
        }
        let in_dir = unit(corner.sub(prev));
        let out_dir = unit(next.sub(corner));
        let enter = corner.sub(in_dir.scale(r));
        let exit = corner.add(out_dir.scale(r));
        out.push(straight_cubic(cursor, enter));
        out.push(quarter_arc_cubic(enter, corner, exit, in_dir, out_dir));
        cursor = exit;
    }
    out.push(straight_cubic(cursor, *points.last().unwrap()));
    out
}

fn unit(p: Point) -> Point {
    let len = p.length();
    if len < 1e-9 {
        Point::zero()
    } else {
        p.scale(1.0 / len)
    }
}

fn straight_cubic(a: Point, b: Point) -> (Point, Point, Point) {
    let d = b.sub(a);
    (a.add(d.scale(1.0 / 3.0)), a.add(d.scale(2.0 / 3.0)), b)
}

/// Cubic approximation of the quarter-circle-ish arc from `enter` to
/// `exit` bending around `corner`, with `in_dir`/`out_dir` the unit
/// tangents on each side. Control points sit `kappa * r` from each
/// endpoint along its own tangent, matching the standard 4-cubic
/// full-circle approximation restricted to one corner.
fn quarter_arc_cubic(
    enter: Point,
    corner: Point,
    exit: Point,
    in_dir: Point,
    out_dir: Point,
) -> (Point, Point, Point) {
    let r = enter.distance_to(corner);
    let c1 = enter.add(in_dir.scale(r * KAPPA));
    let c2 = exit.sub(out_dir.scale(r * KAPPA));
    (c1, c2, exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_drops_collinear_points() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 50.0),
            Point::new(0.0, 100.0),
            Point::new(50.0, 100.0),
        ];
        let simplified = simplify(&pts, 1.0);
        assert_eq!(simplified.len(), 3);
    }

    #[test]
    fn rounded_cubics_straight_line_has_one_segment() {
        let pts = vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)];
        let segs = to_rounded_cubics(&pts, 10.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].2, pts[1]);
    }

    #[test]
    fn rounded_cubics_one_corner_has_three_segments() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 100.0),
            Point::new(100.0, 100.0),
        ];
        let segs = to_rounded_cubics(&pts, 10.0);
        // straight-in, arc, straight-out
        assert_eq!(segs.len(), 3);
        assert_eq!(segs.last().unwrap().2, pts[2]);
    }

    #[test]
    fn rounded_cubics_clamps_radius_to_short_segment() {
        // Second segment is only 4pt long — radius must clamp to 2pt,
        // not the requested 10pt (which would overshoot the endpoint).
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 100.0),
            Point::new(4.0, 100.0),
        ];
        let segs = to_rounded_cubics(&pts, 10.0);
        assert_eq!(segs.len(), 3);
        // Enter point of the arc should be within [0,100-2] on y.
        let arc_start_y = segs[0].2.y;
        assert!((97.9..=98.1).contains(&arc_start_y), "got {arc_start_y}");
    }
}
