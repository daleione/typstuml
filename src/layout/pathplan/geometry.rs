//! Local geometry helpers used only inside `pathplan`. The wider project's
//! geometry module (`crate::layout::geometry`) carries `Point` and box
//! helpers; everything here is the small set of orientation primitives the
//! Lee-Preparata funnel and ear-clip triangulation need.
//!
//! Mirrors `gen/lib/pathplan/shortest__c.java::{ccw, intersects, between}`.

use crate::layout::geometry::Point;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Orient {
    Ccw,
    Cw,
    Collinear,
}

/// Orientation of the directed turn `a → b → c`. Matches dot's `ccw()`
/// (returns 1 / 2 / 3 in the original; we use an enum).
pub(super) fn ccw(a: Point, b: Point, c: Point) -> Orient {
    let d = (a.y - b.y) * (c.x - b.x) - (c.y - b.y) * (a.x - b.x);
    if d > 0.0 {
        Orient::Ccw
    } else if d < 0.0 {
        Orient::Cw
    } else {
        Orient::Collinear
    }
}

/// True iff segments `ab` and `cd` cross or touch. Mirrors `intersects()` in
/// dot — including the collinear-touch case via `between()`.
pub(super) fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let o1 = ccw(a, b, c);
    let o2 = ccw(a, b, d);
    let o3 = ccw(c, d, a);
    let o4 = ccw(c, d, b);

    if o1 == Orient::Collinear
        || o2 == Orient::Collinear
        || o3 == Orient::Collinear
        || o4 == Orient::Collinear
    {
        between(a, b, c) || between(a, b, d) || between(c, d, a) || between(c, d, b)
    } else {
        (o1 == Orient::Ccw) != (o2 == Orient::Ccw)
            && (o3 == Orient::Ccw) != (o4 == Orient::Ccw)
    }
}

/// True iff `c` lies on segment `ab` (inclusive of endpoints). Assumes — and
/// re-checks — collinearity.
pub(super) fn between(a: Point, b: Point, c: Point) -> bool {
    if ccw(a, b, c) != Orient::Collinear {
        return false;
    }
    let p1 = b.sub(a);
    let p2 = c.sub(a);
    let dot = p2.x * p1.x + p2.y * p1.y;
    let p2_len_sq = p2.x * p2.x + p2.y * p2.y;
    let p1_len_sq = p1.x * p1.x + p1.y * p1.y;
    dot >= 0.0 && p2_len_sq <= p1_len_sq
}

/// True iff `p` is on the closed CCW triangle `abc`. Mirrors `pointintri`:
/// for every edge, `p` must not be strictly to the right (CW); collinear is
/// accepted.
pub(super) fn point_in_triangle(p: Point, a: Point, b: Point, c: Point) -> bool {
    let mut sum = 0;
    for (e0, e1) in [(a, b), (b, c), (c, a)] {
        if ccw(e0, e1, p) != Orient::Cw {
            sum += 1;
        }
    }
    sum == 3 || sum == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn ccw_basic() {
        // Counter-clockwise triangle.
        assert_eq!(ccw(pt(0., 0.), pt(1., 0.), pt(0., 1.)), Orient::Ccw);
        // Clockwise triangle.
        assert_eq!(ccw(pt(0., 0.), pt(0., 1.), pt(1., 0.)), Orient::Cw);
        // Collinear.
        assert_eq!(ccw(pt(0., 0.), pt(1., 0.), pt(2., 0.)), Orient::Collinear);
    }

    #[test]
    fn between_collinear_only() {
        assert!(between(pt(0., 0.), pt(2., 0.), pt(1., 0.)));
        assert!(between(pt(0., 0.), pt(2., 0.), pt(0., 0.)));
        assert!(between(pt(0., 0.), pt(2., 0.), pt(2., 0.)));
        // Non-collinear → not between.
        assert!(!between(pt(0., 0.), pt(2., 0.), pt(1., 1.)));
        // Outside the segment → not between.
        assert!(!between(pt(0., 0.), pt(2., 0.), pt(3., 0.)));
        assert!(!between(pt(0., 0.), pt(2., 0.), pt(-1., 0.)));
    }

    #[test]
    fn segments_intersect_proper_crossing() {
        // X-shaped crossing.
        assert!(segments_intersect(
            pt(0., 0.),
            pt(1., 1.),
            pt(0., 1.),
            pt(1., 0.)
        ));
    }

    #[test]
    fn segments_intersect_disjoint() {
        assert!(!segments_intersect(
            pt(0., 0.),
            pt(1., 0.),
            pt(2., 0.),
            pt(3., 0.)
        ));
        assert!(!segments_intersect(
            pt(0., 0.),
            pt(1., 0.),
            pt(0., 1.),
            pt(1., 1.)
        ));
    }

    #[test]
    fn segments_intersect_collinear_touch() {
        // Touching at a single endpoint → counts as intersection.
        assert!(segments_intersect(
            pt(0., 0.),
            pt(1., 0.),
            pt(1., 0.),
            pt(2., 0.)
        ));
        // Overlapping segments.
        assert!(segments_intersect(
            pt(0., 0.),
            pt(2., 0.),
            pt(1., 0.),
            pt(3., 0.)
        ));
    }

    #[test]
    fn point_in_triangle_interior_and_edge() {
        let a = pt(0., 0.);
        let b = pt(2., 0.);
        let c = pt(0., 2.);
        assert!(point_in_triangle(pt(0.5, 0.5), a, b, c));
        // Vertex.
        assert!(point_in_triangle(a, a, b, c));
        // On edge.
        assert!(point_in_triangle(pt(1., 0.), a, b, c));
        // Outside.
        assert!(!point_in_triangle(pt(2., 2.), a, b, c));
    }
}
