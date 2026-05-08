//! Simple-polygon container with CCW orientation, ear-clip triangulation,
//! and triangle adjacency. Mirrors the geometry the C `pathplan` operates
//! on (`Ppoly_t` plus the triangle list / `connecttris` linkage).

use super::geometry::{ccw, point_in_triangle, segments_intersect, Orient};
use super::PathError;
use crate::layout::geometry::Point;

/// A simple closed polygon stored CCW. Construction validates: at least
/// three distinct vertices, no edge crossings; consecutive duplicates and a
/// trailing wrap-around are silently dropped, and CW input is reversed.
#[derive(Debug, Clone)]
pub struct Polygon {
    vertices: Vec<Point>,
}

impl Polygon {
    pub fn new(mut vertices: Vec<Point>) -> Result<Self, PathError> {
        // Drop trivial duplicates so caller doesn't have to be precise.
        vertices.dedup();
        if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
            vertices.pop();
        }
        if vertices.len() < 3 {
            return Err(PathError::PolygonTooSmall(vertices.len()));
        }

        let n = vertices.len();
        for i in 0..n {
            let a = vertices[i];
            let b = vertices[(i + 1) % n];
            // Skip the edge itself and its two neighbours (which always
            // share a vertex with it).
            for j in (i + 1)..n {
                if j == i || (j + 1) % n == i || (i + 1) % n == j {
                    continue;
                }
                let c = vertices[j];
                let d = vertices[(j + 1) % n];
                if segments_intersect(a, b, c, d) {
                    return Err(PathError::PolygonSelfIntersecting);
                }
            }
        }

        if signed_area(&vertices) < 0.0 {
            vertices.reverse();
        }

        Ok(Polygon { vertices })
    }

    pub fn vertices(&self) -> &[Point] {
        &self.vertices
    }

    pub fn len(&self) -> usize {
        self.vertices.len()
    }

    /// Even-odd ray-cast containment. Boundary points may report either
    /// way depending on the ray; for `pathplan`'s purposes the triangle
    /// containment test (`shortest::find_triangle`) is the authoritative
    /// "is the endpoint usable?" check.
    pub fn contains(&self, p: Point) -> bool {
        let n = self.vertices.len();
        if n < 3 {
            return false;
        }
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let vi = self.vertices[i];
            let vj = self.vertices[j];
            if (vi.y > p.y) != (vj.y > p.y) {
                let xint = vj.x + (p.y - vj.y) * (vi.x - vj.x) / (vi.y - vj.y);
                if p.x < xint {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }

    /// Triangulate via ear-clipping. Each triangle's `verts` are indices
    /// into `self.vertices`. Adjacency (`adj[i]`) is filled in: triangles
    /// that share the edge `(verts[i], verts[(i+1) % 3])` link to each
    /// other.
    pub(super) fn triangulate(&self) -> Vec<Triangle> {
        let n = self.vertices.len();
        if n < 3 {
            return Vec::new();
        }
        let mut idx: Vec<usize> = (0..n).collect();
        let mut tris = Vec::with_capacity(n - 2);

        while idx.len() > 3 {
            let m = idx.len();
            let mut clipped = false;
            for i in 0..m {
                if self.is_diagonal(&idx, i) {
                    let i0 = idx[i];
                    let i1 = idx[(i + 1) % m];
                    let i2 = idx[(i + 2) % m];
                    tris.push(Triangle::new(i0, i1, i2));
                    idx.remove((i + 1) % m);
                    clipped = true;
                    break;
                }
            }
            if !clipped {
                // Should not happen on a simple polygon, but bail rather
                // than spin forever.
                return Vec::new();
            }
        }
        if idx.len() == 3 {
            tris.push(Triangle::new(idx[0], idx[1], idx[2]));
        }

        connect_adjacency(&mut tris);
        tris
    }

    /// Mirrors `isdiagonal(pnli, pnli+2)`: tests that the diagonal from
    /// the i-th surviving vertex to its (i+2)-th neighbour is interior to
    /// the polygon and doesn't cross any other surviving edge. If true,
    /// removing the (i+1)-th vertex cuts a valid ear.
    fn is_diagonal(&self, idx: &[usize], i: usize) -> bool {
        let m = idx.len();
        let i_prev = (i + m - 1) % m;
        let i_next = (i + 1) % m;
        let i_target = (i + 2) % m;
        let a = self.vertices[idx[i]];
        let prev = self.vertices[idx[i_prev]];
        let next = self.vertices[idx[i_next]];
        let target = self.vertices[idx[i_target]];

        // Half-plane test at vertex `a` — is the diagonal a→target interior?
        let interior = if ccw(prev, a, next) == Orient::Ccw {
            // Convex corner.
            ccw(a, target, prev) == Orient::Ccw && ccw(target, a, next) == Orient::Ccw
        } else {
            // Reflex (or collinear) corner: the diagonal is interior iff it
            // exits on the CW side of (a, next).
            ccw(a, target, next) == Orient::Cw
        };
        if !interior {
            return false;
        }

        // No surviving edge may cross the diagonal.
        for j in 0..m {
            let jp = (j + 1) % m;
            if j == i || jp == i || j == i_target || jp == i_target {
                continue;
            }
            let pj = self.vertices[idx[j]];
            let pjp = self.vertices[idx[jp]];
            if segments_intersect(a, target, pj, pjp) {
                return false;
            }
        }
        true
    }
}

fn signed_area(verts: &[Point]) -> f64 {
    let n = verts.len();
    let mut s = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        s += verts[i].x * verts[j].y - verts[j].x * verts[i].y;
    }
    s / 2.0
}

#[derive(Debug, Clone)]
pub(super) struct Triangle {
    /// Polygon vertex indices in CCW order.
    pub verts: [usize; 3],
    /// `adj[i]` = index of the triangle sharing edge
    /// `(verts[i], verts[(i+1) % 3])`, if any.
    pub adj: [Option<usize>; 3],
}

impl Triangle {
    fn new(a: usize, b: usize, c: usize) -> Self {
        Triangle {
            verts: [a, b, c],
            adj: [None; 3],
        }
    }

    pub fn edge(&self, i: usize) -> (usize, usize) {
        (self.verts[i], self.verts[(i + 1) % 3])
    }

    /// Index `ei` such that `verts[ei]` and `verts[(ei+1) % 3]` is the edge
    /// shared with `other`. Returns `None` if the two triangles aren't
    /// recorded as neighbours.
    pub fn edge_to(&self, other: usize) -> Option<usize> {
        for ei in 0..3 {
            if self.adj[ei] == Some(other) {
                return Some(ei);
            }
        }
        None
    }
}

fn connect_adjacency(tris: &mut [Triangle]) {
    let n = tris.len();
    for i in 0..n {
        for j in (i + 1)..n {
            for ei in 0..3 {
                let (a0, a1) = tris[i].edge(ei);
                for ej in 0..3 {
                    let (b0, b1) = tris[j].edge(ej);
                    if (a0 == b0 && a1 == b1) || (a0 == b1 && a1 == b0) {
                        tris[i].adj[ei] = Some(j);
                        tris[j].adj[ej] = Some(i);
                    }
                }
            }
        }
    }
}

/// Helper for `shortest::find_triangle`. Lives here so the triangle
/// container also carries containment.
pub(super) fn triangle_contains(tri: &Triangle, verts: &[Point], p: Point) -> bool {
    let a = verts[tri.verts[0]];
    let b = verts[tri.verts[1]];
    let c = verts[tri.verts[2]];
    point_in_triangle(p, a, b, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn rejects_too_few_vertices() {
        assert!(matches!(
            Polygon::new(vec![pt(0., 0.), pt(1., 0.)]),
            Err(PathError::PolygonTooSmall(2))
        ));
    }

    #[test]
    fn drops_trailing_wraparound() {
        let p = Polygon::new(vec![pt(0., 0.), pt(1., 0.), pt(0., 1.), pt(0., 0.)]).unwrap();
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn corrects_cw_to_ccw() {
        // CW input should be flipped to CCW.
        let p = Polygon::new(vec![pt(0., 0.), pt(0., 1.), pt(1., 0.)]).unwrap();
        // After correction, signed area is positive (CCW).
        assert!(signed_area(p.vertices()) > 0.0);
    }

    #[test]
    fn rejects_self_intersecting() {
        // Bow-tie quadrilateral.
        let result = Polygon::new(vec![pt(0., 0.), pt(2., 2.), pt(2., 0.), pt(0., 2.)]);
        assert!(matches!(result, Err(PathError::PolygonSelfIntersecting)));
    }

    #[test]
    fn contains_interior_and_excludes_exterior() {
        // Unit square.
        let p =
            Polygon::new(vec![pt(0., 0.), pt(2., 0.), pt(2., 2.), pt(0., 2.)]).unwrap();
        assert!(p.contains(pt(1., 1.)));
        assert!(!p.contains(pt(3., 1.)));
        assert!(!p.contains(pt(-1., 1.)));
    }

    #[test]
    fn triangulates_square_into_two_triangles() {
        let p =
            Polygon::new(vec![pt(0., 0.), pt(2., 0.), pt(2., 2.), pt(0., 2.)]).unwrap();
        let tris = p.triangulate();
        assert_eq!(tris.len(), 2);
        // Each triangle must have at least one neighbour (the diagonal).
        let total_adj: usize = tris
            .iter()
            .map(|t| t.adj.iter().filter(|x| x.is_some()).count())
            .sum();
        assert_eq!(total_adj, 2, "diagonal must connect both triangles");
    }

    #[test]
    fn triangulates_concave_l_shape() {
        // L-shape: 6 vertices, expect 4 triangles.
        let p = Polygon::new(vec![
            pt(0., 0.),
            pt(4., 0.),
            pt(4., 2.),
            pt(2., 2.),
            pt(2., 4.),
            pt(0., 4.),
        ])
        .unwrap();
        let tris = p.triangulate();
        assert_eq!(tris.len(), 4);
    }
}
