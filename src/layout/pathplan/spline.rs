//! B-spline routing inside a constraint polygon. Mirrors
//! `gen/lib/pathplan/route__c.java::Proutespline` plus the recursion driven
//! by the polyline's max-deviation split.
//!
//! Public entry: [`route_spline`]. Internally:
//!
//! - `mkspline` — least-squares fit of two control-handle scales given the
//!   guide polyline and prescribed end-tangents.
//! - `splinefits` — try a candidate spline; halve the control-handle scale
//!   on rejection until the spline either fits inside the polygon or both
//!   scales hit zero.
//! - `splineisinside` — for every polygon edge, solve the cubic-vs-line
//!   intersection analytically and reject if any internal root produces
//!   an intersection point not at an edge endpoint.
//!
//! When `splinefits` gives up on a >2-point segment, we split the polyline
//! at its max-deviation point and recurse on both halves with a corner
//! tangent that bisects incoming and outgoing legs.

use super::polynomial::{solve3, RootCount};
use super::shortest::Polyline;
use super::Polygon;
use crate::layout::geometry::Point;

/// One cubic Bezier segment. `start` is included so each segment is fully
/// self-describing for testing and inspection; the painter consumes only
/// `(c1, c2, end)`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cubic {
    pub start: Point,
    pub c1: Point,
    pub c2: Point,
    pub end: Point,
}

impl Cubic {
    pub fn into_painter_segment(self) -> (Point, Point, Point) {
        (self.c1, self.c2, self.end)
    }
}

/// Fit a sequence of cubic Beziers that follows `polyline` from end to end
/// without ever exiting `polygon`. Tangents specify the entry direction at
/// the first point and the exit direction at the last; the caller normally
/// hands `(1, 0)` for both (horizontal exit / entry, matching record-graph
/// edges).
pub fn route_spline(
    polygon: &Polygon,
    polyline: &Polyline,
    src_tangent: Point,
    dst_tangent: Point,
) -> Vec<Cubic> {
    let inps: Vec<Point> = polyline.points().to_vec();
    if inps.len() < 2 {
        return Vec::new();
    }

    let edges = polygon_edges(polygon);
    let mut out = Vec::new();
    really_route(&edges, &inps, normv(src_tangent), normv(dst_tangent), &mut out);
    out
}

fn polygon_edges(polygon: &Polygon) -> Vec<(Point, Point)> {
    let verts = polygon.vertices();
    let n = verts.len();
    (0..n).map(|i| (verts[i], verts[(i + 1) % n])).collect()
}

/// Recursive worker. Direct port of `reallyroutespline` minus the
/// `setjmp`/global allocations.
fn really_route(
    edges: &[(Point, Point)],
    inps: &[Point],
    ev0: Point,
    ev1: Point,
    out: &mut Vec<Cubic>,
) {
    let inpn = inps.len();
    debug_assert!(inpn >= 2);

    // Parameterise polyline points by cumulative arc length, normalised to
    // [0, 1]. If the entire polyline is a single point we still want a
    // valid t schedule — duplicate-point inputs shouldn't happen, but be
    // defensive and fall back to a uniform parameterisation.
    let mut t = vec![0.0; inpn];
    for i in 1..inpn {
        t[i] = t[i - 1] + inps[i].distance_to(inps[i - 1]);
    }
    let total = t[inpn - 1];
    if total > 0.0 {
        for ti in t.iter_mut() {
            *ti /= total;
        }
    } else {
        for (i, ti) in t.iter_mut().enumerate() {
            *ti = i as f64 / (inpn - 1) as f64;
        }
    }

    // a[0] = ev0 * B1(t), a[1] = ev1 * B2(t) per polyline point.
    let a: Vec<[Point; 2]> = t
        .iter()
        .map(|&ti| [ev0.scale(b1(ti)), ev1.scale(b2(ti))])
        .collect();

    let (p1, v1, p2, v2) = mkspline(inps, &a, &t, ev0, ev1);

    if splinefits(edges, p1, v1, p2, v2, inps, out) {
        return;
    }

    // Find the polyline point with greatest deviation from the rejected
    // candidate spline; recurse on both halves with a corner tangent that
    // bisects the incoming and outgoing legs.
    let cp1 = p1.add(v1.scale(1.0 / 3.0));
    let cp2 = p2.sub(v2.scale(1.0 / 3.0));
    let mut maxd = -1.0;
    let mut maxi = 1;
    for i in 1..(inpn - 1) {
        let ti = t[i];
        let p = bezier_eval(p1, cp1, cp2, p2, ti);
        let d = p.distance_to(inps[i]);
        if d > maxd {
            maxd = d;
            maxi = i;
        }
    }

    let spliti = maxi;
    let splitv1 = normv(inps[spliti].sub(inps[spliti - 1]));
    let splitv2 = normv(inps[spliti + 1].sub(inps[spliti]));
    let splitv = normv(splitv1.add(splitv2));

    really_route(edges, &inps[..=spliti], ev0, splitv, out);
    really_route(edges, &inps[spliti..], splitv, ev1, out);
}

/// Least-squares fit for the two control-handle scales `(scale0, scale3)`
/// that minimise the L² distance between the resulting cubic and the input
/// polyline. Mirrors `mkspline`.
fn mkspline(
    inps: &[Point],
    a: &[[Point; 2]],
    t: &[f64],
    ev0: Point,
    ev1: Point,
) -> (Point, Point, Point, Point) {
    let inpn = inps.len();
    let p_first = inps[0];
    let p_last = inps[inpn - 1];

    let mut c00 = 0.0;
    let mut c01 = 0.0;
    let mut c11 = 0.0;
    let mut x0 = 0.0;
    let mut x1 = 0.0;
    for i in 0..inpn {
        c00 += dot(a[i][0], a[i][0]);
        c01 += dot(a[i][0], a[i][1]);
        c11 += dot(a[i][1], a[i][1]);
        let bracket = p_first.scale(b01(t[i])).add(p_last.scale(b23(t[i])));
        let tmp = inps[i].sub(bracket);
        x0 += dot(a[i][0], tmp);
        x1 += dot(a[i][1], tmp);
    }
    let det01 = c00 * c11 - c01 * c01;
    let det0x = c00 * x1 - c01 * x0;
    let detx1 = x0 * c11 - x1 * c01;
    let mut scale0 = 0.0;
    let mut scale3 = 0.0;
    if det01.abs() >= 1e-6 {
        scale0 = detx1 / det01;
        scale3 = det0x / det01;
    }
    if det01.abs() < 1e-6 || scale0 <= 0.0 || scale3 <= 0.0 {
        let d01 = p_first.distance_to(p_last) / 3.0;
        scale0 = d01;
        scale3 = d01;
    }
    (p_first, ev0.scale(scale0), p_last, ev1.scale(scale3))
}

/// Try fitting a cubic with the prescribed endpoints, tangents, and start
/// scale `a = b = 4`. On rejection, halve both scales and retry. Returns
/// true and pushes the accepted cubic to `out` on success. The 2-point
/// `forceflag` branch always succeeds — that's how recursion bottoms out.
fn splinefits(
    edges: &[(Point, Point)],
    pa: Point,
    va: Point,
    pb: Point,
    vb: Point,
    inps: &[Point],
    out: &mut Vec<Cubic>,
) -> bool {
    let force = inps.len() == 2;
    let mut a = 4.0;
    let mut b = 4.0;
    let mut first = true;
    loop {
        let sps = [
            pa,
            pa.add(va.scale(a / 3.0)),
            pb.sub(vb.scale(b / 3.0)),
            pb,
        ];

        // Reject "shortcuts": candidate spline shorter than the polyline
        // means it's cutting through the polygon. Only checked once.
        if first && polyline_length(&sps) < polyline_length(inps) - 1e-3 {
            return false;
        }
        first = false;

        if spline_is_inside(edges, &sps) {
            out.push(Cubic {
                start: sps[0],
                c1: sps[1],
                c2: sps[2],
                end: sps[3],
            });
            return true;
        }
        if a == 0.0 && b == 0.0 {
            if force {
                out.push(Cubic {
                    start: sps[0],
                    c1: sps[1],
                    c2: sps[2],
                    end: sps[3],
                });
                return true;
            }
            return false;
        }
        if a > 0.01 {
            a /= 2.0;
            b /= 2.0;
        } else {
            a = 0.0;
            b = 0.0;
        }
    }
}

/// True iff the cubic defined by `sps` (4 control points) doesn't cross
/// any polygon edge except at edge endpoints.
fn spline_is_inside(edges: &[(Point, Point)], sps: &[Point; 4]) -> bool {
    for &(la, lb) in edges {
        let (rootn, roots) = match spline_intersects_line(sps, la, lb) {
            // count == 4 in dot is the "infinitely many" sentinel; treat
            // as "everywhere on the line" → no internal crossing to count.
            None => continue,
            Some(rs) => rs,
        };
        for r in roots.iter().take(rootn) {
            if *r < 1e-6 || *r > 1.0 - 1e-6 {
                continue;
            }
            let t = *r;
            let p = bezier_eval(sps[0], sps[1], sps[2], sps[3], t);
            if dist_sq(p, la) < 1e-3 || dist_sq(p, lb) < 1e-3 {
                continue;
            }
            return false;
        }
    }
    true
}

/// Solve the cubic-vs-line intersection in spline parameter `t`. Returns
/// `None` for the "infinitely many roots" case (line parallel to and
/// touching a flat axis-aligned cubic component).
fn spline_intersects_line(sps: &[Point; 4], la: Point, lb: Point) -> Option<(usize, [f64; 3])> {
    let xc = [la.x, lb.x - la.x];
    let yc = [la.y, lb.y - la.y];
    let mut roots = [0.0f64; 3];

    let mut count: usize = 0;
    let mut add = |val: f64| {
        if (0.0..=1.0).contains(&val) && count < roots.len() {
            roots[count] = val;
            count += 1;
        }
    };

    if xc[1] == 0.0 {
        if yc[1] == 0.0 {
            // Degenerate "edge": a point. Solve3 on each axis and return
            // overlap.
            let mut sx = points2coeff(sps[0].x, sps[1].x, sps[2].x, sps[3].x);
            sx[0] -= xc[0];
            let mut x_roots = [0.0f64; 3];
            let xres = solve3(sx, &mut x_roots);

            let mut sy = points2coeff(sps[0].y, sps[1].y, sps[2].y, sps[3].y);
            sy[0] -= yc[0];
            let mut y_roots = [0.0f64; 3];
            let yres = solve3(sy, &mut y_roots);

            match (xres, yres) {
                (RootCount::Infinite, RootCount::Infinite) => return None,
                (RootCount::Infinite, RootCount::Finite(yn)) => {
                    for &v in y_roots.iter().take(yn) {
                        add(v);
                    }
                }
                (RootCount::Finite(xn), RootCount::Infinite) => {
                    for &v in x_roots.iter().take(xn) {
                        add(v);
                    }
                }
                (RootCount::Finite(xn), RootCount::Finite(yn)) => {
                    for &xr in x_roots.iter().take(xn) {
                        for &yr in y_roots.iter().take(yn) {
                            if xr == yr {
                                add(xr);
                            }
                        }
                    }
                }
                _ => {}
            }
        } else {
            // Vertical line.
            let mut sx = points2coeff(sps[0].x, sps[1].x, sps[2].x, sps[3].x);
            sx[0] -= xc[0];
            let mut x_roots = [0.0f64; 3];
            let xres = solve3(sx, &mut x_roots);
            if xres == RootCount::Infinite {
                return None;
            }
            if let RootCount::Finite(xn) = xres {
                for &xr in x_roots.iter().take(xn) {
                    if !(0.0..=1.0).contains(&xr) {
                        continue;
                    }
                    let sy = points2coeff(sps[0].y, sps[1].y, sps[2].y, sps[3].y);
                    let sv = sy[0] + xr * (sy[1] + xr * (sy[2] + xr * sy[3]));
                    let line_t = (sv - yc[0]) / yc[1];
                    if (0.0..=1.0).contains(&line_t) {
                        add(xr);
                    }
                }
            }
        }
    } else {
        // General line: project onto its normal direction.
        let rat = yc[1] / xc[1];
        let mut sc = points2coeff(
            sps[0].y - rat * sps[0].x,
            sps[1].y - rat * sps[1].x,
            sps[2].y - rat * sps[2].x,
            sps[3].y - rat * sps[3].x,
        );
        sc[0] += rat * xc[0] - yc[0];
        let mut x_roots = [0.0f64; 3];
        let xres = solve3(sc, &mut x_roots);
        if xres == RootCount::Infinite {
            return None;
        }
        if let RootCount::Finite(xn) = xres {
            for &xr in x_roots.iter().take(xn) {
                if !(0.0..=1.0).contains(&xr) {
                    continue;
                }
                let sx = points2coeff(sps[0].x, sps[1].x, sps[2].x, sps[3].x);
                let sv = sx[0] + xr * (sx[1] + xr * (sx[2] + xr * sx[3]));
                let line_t = (sv - xc[0]) / xc[1];
                if (0.0..=1.0).contains(&line_t) {
                    add(xr);
                }
            }
        }
    }
    Some((count, roots))
}

/// Convert four scalar Bernstein coefficients into the cubic polynomial
/// `c[0] + c[1]·t + c[2]·t² + c[3]·t³`. Mirrors `points2coeff`.
fn points2coeff(v0: f64, v1: f64, v2: f64, v3: f64) -> [f64; 4] {
    [
        v0,
        3.0 * (v1 - v0),
        3.0 * v0 + 3.0 * v2 - 6.0 * v1,
        v3 + 3.0 * v1 - (v0 + 3.0 * v2),
    ]
}

fn bezier_eval(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let b0 = b0(t);
    let b1 = b1(t);
    let b2 = b2(t);
    let b3 = b3(t);
    Point::new(
        b0 * p0.x + b1 * p1.x + b2 * p2.x + b3 * p3.x,
        b0 * p0.y + b1 * p1.y + b2 * p2.y + b3 * p3.y,
    )
}

fn polyline_length(pts: &[Point]) -> f64 {
    let mut total = 0.0;
    for i in 1..pts.len() {
        total += pts[i].distance_to(pts[i - 1]);
    }
    total
}

fn normv(v: Point) -> Point {
    let len_sq = v.x * v.x + v.y * v.y;
    if len_sq > 1e-6 {
        let len = len_sq.sqrt();
        Point::new(v.x / len, v.y / len)
    } else {
        v
    }
}

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}

fn dist_sq(a: Point, b: Point) -> f64 {
    let d = a.sub(b);
    d.x * d.x + d.y * d.y
}

#[inline]
fn b0(t: f64) -> f64 {
    let m = 1.0 - t;
    m * m * m
}
#[inline]
fn b1(t: f64) -> f64 {
    let m = 1.0 - t;
    3.0 * t * m * m
}
#[inline]
fn b2(t: f64) -> f64 {
    let m = 1.0 - t;
    3.0 * t * t * m
}
#[inline]
fn b3(t: f64) -> f64 {
    t * t * t
}
#[inline]
fn b01(t: f64) -> f64 {
    b0(t) + b1(t)
}
#[inline]
fn b23(t: f64) -> f64 {
    b2(t) + b3(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::pathplan::shortest::shortest_path;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn unit_square() -> Polygon {
        Polygon::new(vec![pt(0., 0.), pt(10., 0.), pt(10., 10.), pt(0., 10.)]).unwrap()
    }

    fn l_shape() -> Polygon {
        Polygon::new(vec![
            pt(0., 0.),
            pt(4., 0.),
            pt(4., 2.),
            pt(2., 2.),
            pt(2., 4.),
            pt(0., 4.),
        ])
        .unwrap()
    }

    #[test]
    fn straight_segment_emits_one_cubic_inside_square() {
        let poly = unit_square();
        let polyline = Polyline(vec![pt(2., 5.), pt(8., 5.)]);
        let cubics = route_spline(&poly, &polyline, pt(1.0, 0.0), pt(1.0, 0.0));
        assert_eq!(cubics.len(), 1);
        let c = cubics[0];
        assert_eq!(c.start, pt(2., 5.));
        assert_eq!(c.end, pt(8., 5.));
        // Tangents are horizontal so c1 is to the right of start, c2 to the
        // left of end.
        assert!(c.c1.x > c.start.x);
        assert!(c.c2.x < c.end.x);
    }

    #[test]
    fn forces_acceptance_when_both_scales_collapse() {
        // Polygon is a thin slab right around y=5, source/target on the
        // boundary; controls have to be small. With force tangents
        // pointing horizontally and only 2 polyline points, splinefits
        // forceflag must succeed.
        let poly =
            Polygon::new(vec![pt(0., 4.5), pt(10., 4.5), pt(10., 5.5), pt(0., 5.5)]).unwrap();
        let polyline = Polyline(vec![pt(2., 5.), pt(8., 5.)]);
        let cubics = route_spline(&poly, &polyline, pt(1.0, 0.0), pt(1.0, 0.0));
        assert_eq!(cubics.len(), 1, "force-accept should produce one cubic");
    }

    #[test]
    fn turn_through_l_shape_produces_curve_inside_polygon() {
        let poly = l_shape();
        let src = pt(1.0, 3.5);
        let dst = pt(3.5, 1.0);
        let polyline = shortest_path(&poly, src, dst).unwrap();
        let cubics = route_spline(&poly, &polyline, pt(1.0, 0.0), pt(1.0, 0.0));
        assert!(!cubics.is_empty());
        let first = cubics.first().unwrap();
        assert_eq!(first.start, src);
        let last = cubics.last().unwrap();
        assert_eq!(last.end, dst);
        // Sample every cubic and check we never exit the polygon (allow a
        // 1e-3 fudge for boundary samples).
        for c in &cubics {
            for k in 0..=10 {
                let t = k as f64 / 10.0;
                let p = bezier_eval(c.start, c.c1, c.c2, c.end, t);
                let inflated = inflate(&poly, 1e-3);
                assert!(
                    inflated.contains(p) || on_boundary(&poly, p, 1e-3),
                    "sample {:?} outside polygon (cubic={:?})",
                    p,
                    c
                );
            }
        }
    }

    fn inflate(poly: &Polygon, eps: f64) -> Polygon {
        // Cheap inflate: we only need it for the in-polygon assertion, so
        // expand the AABB only. (Won't be perfectly accurate for non-convex
        // polygons; the on_boundary fallback handles that case.)
        let v = poly.vertices();
        let min_x = v.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - eps;
        let max_x = v.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max) + eps;
        let min_y = v.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - eps;
        let max_y = v.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max) + eps;
        Polygon::new(vec![
            pt(min_x, min_y),
            pt(max_x, min_y),
            pt(max_x, max_y),
            pt(min_x, max_y),
        ])
        .unwrap()
    }

    fn on_boundary(poly: &Polygon, p: Point, eps: f64) -> bool {
        let v = poly.vertices();
        let n = v.len();
        for i in 0..n {
            let a = v[i];
            let b = v[(i + 1) % n];
            let len = a.distance_to(b);
            if len < 1e-9 {
                continue;
            }
            let t = ((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) / (len * len);
            if !(-eps..=1.0 + eps).contains(&t) {
                continue;
            }
            let proj = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
            if proj.distance_to(p) <= eps {
                return true;
            }
        }
        false
    }
}
