//! Lee-Preparata shortest path inside a simple polygon. Mirrors
//! `gen/lib/pathplan/shortest__c.java::Pshortestpath`. The C original spreads
//! state across module-level globals (`pnls`, `pnlps`, `dq`, `tris`); the
//! Rust port localises everything to a `Funnel` struct and a recursive DFS
//! over triangle adjacency.

use super::geometry::{ccw, Orient};
use super::polygon::{triangle_contains, Polygon, Triangle};
use super::PathError;
use crate::layout::geometry::Point;

/// Open polyline (≥ 2 points). Output of `shortest_path`.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline(pub Vec<Point>);

impl Polyline {
    pub fn points(&self) -> &[Point] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// Shortest path from `src` to `dst` inside `polygon`. Both endpoints must
/// lie inside the polygon (or on its boundary).
pub fn shortest_path(
    polygon: &Polygon,
    src: Point,
    dst: Point,
) -> Result<Polyline, PathError> {
    if polygon.len() < 3 {
        return Err(PathError::PolygonTooSmall(polygon.len()));
    }

    let tris = polygon.triangulate();
    if tris.is_empty() {
        return Err(PathError::PolygonTooSmall(polygon.len()));
    }

    let verts = polygon.vertices();
    let src_tri = find_triangle(&tris, verts, src).ok_or(PathError::SourceOutside)?;
    let dst_tri = find_triangle(&tris, verts, dst).ok_or(PathError::DestinationOutside)?;

    if src_tri == dst_tri {
        return Ok(Polyline(vec![src, dst]));
    }

    let path = mark_triangle_path(&tris, src_tri, dst_tri).ok_or(PathError::NoTrianglePath)?;
    Ok(Polyline(funnel(verts, &tris, &path, src, dst)))
}

fn find_triangle(tris: &[Triangle], verts: &[Point], p: Point) -> Option<usize> {
    tris.iter()
        .position(|t| triangle_contains(t, verts, p))
}

/// DFS through triangle adjacency to find a path from `src_tri` to
/// `dst_tri`. Returns the triangle indices in order.
fn mark_triangle_path(
    tris: &[Triangle],
    src_tri: usize,
    dst_tri: usize,
) -> Option<Vec<usize>> {
    let mut visited = vec![false; tris.len()];
    let mut path = Vec::new();
    if dfs(tris, src_tri, dst_tri, &mut visited, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn dfs(
    tris: &[Triangle],
    cur: usize,
    dst: usize,
    visited: &mut [bool],
    path: &mut Vec<usize>,
) -> bool {
    if visited[cur] {
        return false;
    }
    visited[cur] = true;
    path.push(cur);
    if cur == dst {
        return true;
    }
    for &neighbour in tris[cur].adj.iter().flatten() {
        if dfs(tris, neighbour, dst, visited, path) {
            return true;
        }
    }
    path.pop();
    false
}

/// The funnel/dq itself. Mirrors Smetana's `dq` plus `link` chain.
///
/// Geometric interpretation: as the funnel sweeps through the triangle path
/// it tracks the "left" and "right" walls of the corridor. New apex
/// candidates push to the front (right wall) or back (left wall). When the
/// funnel collapses past its current apex, `splitdq` discards the obsolete
/// tail of one wall and promotes a new apex; the discarded tail's `link`
/// pointers are still alive and form the chain back to source. The output
/// path is read by walking `link` from the destination's slot.
struct Funnel {
    pts: Vec<Point>,
    /// `link[i] = Some(j)`: arena slot `i`'s next-toward-source is slot `j`.
    /// `None` for the source itself.
    link: Vec<Option<usize>>,
    /// Pre-allocated deque: arena indices at deque positions
    /// `[front .. = back]` form the active funnel.
    dq: Vec<usize>,
    front: isize,
    back: isize,
    apex: isize,
}

impl Funnel {
    fn new(verts: &[Point], src: Point, dst: Point) -> (Self, usize, usize) {
        let n = verts.len();
        let src_idx = n;
        let dst_idx = n + 1;
        let mut pts = Vec::with_capacity(n + 2);
        pts.extend_from_slice(verts);
        pts.push(src);
        pts.push(dst);
        let cap = 2 * (n + 2);
        let mid = (cap / 2) as isize;
        let funnel = Funnel {
            pts,
            link: vec![None; n + 2],
            dq: vec![usize::MAX; cap],
            front: mid,
            back: mid - 1, // empty: back < front
            apex: mid,
        };
        (funnel, src_idx, dst_idx)
    }

    fn is_empty(&self) -> bool {
        self.back < self.front
    }

    fn front_pt(&self) -> usize {
        self.dq[self.front as usize]
    }

    fn back_pt(&self) -> usize {
        self.dq[self.back as usize]
    }

    fn push_front(&mut self, idx: usize) {
        if !self.is_empty() {
            self.link[idx] = Some(self.front_pt());
        }
        self.front -= 1;
        self.dq[self.front as usize] = idx;
    }

    fn push_back(&mut self, idx: usize) {
        if !self.is_empty() {
            self.link[idx] = Some(self.back_pt());
        }
        self.back += 1;
        self.dq[self.back as usize] = idx;
    }

    fn split_front(&mut self, index: isize) {
        // Smetana's splitdq(side=1, index): discards the back beyond `index`.
        self.back = index;
    }

    fn split_back(&mut self, index: isize) {
        // splitdq(side=2, index): discards the front before `index`.
        self.front = index;
    }

    /// Find the deque position to split at when inserting `p`.
    fn find_split(&self, p: Point) -> isize {
        let mut idx = self.front;
        while idx < self.apex {
            let pi = self.dq[idx as usize];
            let pi1 = self.dq[(idx + 1) as usize];
            if ccw(self.pts[pi1], self.pts[pi], p) == Orient::Ccw {
                return idx;
            }
            idx += 1;
        }
        let mut idx = self.back;
        while idx > self.apex {
            let pi = self.dq[idx as usize];
            let pim = self.dq[(idx - 1) as usize];
            if ccw(self.pts[pim], self.pts[pi], p) == Orient::Cw {
                return idx;
            }
            idx -= 1;
        }
        self.apex
    }

    /// Walk the link chain from `start` back to its source-rooted terminus,
    /// collecting points in source-to-`start` order.
    fn chain_from(&self, start: usize) -> Vec<Point> {
        let mut chain = Vec::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            chain.push(self.pts[c]);
            cur = self.link[c];
        }
        chain.reverse();
        chain
    }
}

fn funnel(
    verts: &[Point],
    tris: &[Triangle],
    path: &[usize],
    src: Point,
    dst: Point,
) -> Vec<Point> {
    let (mut f, src_idx, dst_idx) = Funnel::new(verts, src, dst);
    f.push_front(src_idx);
    f.apex = f.front;

    for (path_pos, &trii) in path.iter().enumerate() {
        let is_last = path_pos == path.len() - 1;

        // Identify the two endpoints of the exit edge (or, for the last
        // triangle, the destination + funnel-back).
        let (lpnlp, rpnlp) = if is_last {
            // Mirror of pathplan/shortest.c's "last triangle" branch. The
            // Smetana port references the back twice; we follow the
            // documented intent: dst replaces the third-vertex slot, the
            // funnel's back stays as the other endpoint, and orientation
            // decides which is left vs right.
            let back = f.back_pt();
            if ccw(dst, f.pts[f.front_pt()], f.pts[back]) == Orient::Ccw {
                (back, dst_idx)
            } else {
                (dst_idx, back)
            }
        } else {
            let next_trii = path[path_pos + 1];
            let tri = &tris[trii];
            let ei = tri
                .edge_to(next_trii)
                .expect("triangle path has unconnected step");
            let (e0, e1) = tri.edge(ei);
            let third = tri.verts[(ei + 2) % 3];
            // For a CCW polygon ccw(e0, third, e1) is CW, so the else
            // branch fires; the if-branch covers degenerate orientation as
            // documented in dot's source.
            if ccw(verts[e0], verts[third], verts[e1]) == Orient::Ccw {
                (e1, e0)
            } else {
                (e0, e1)
            }
        };

        if path_pos == 0 {
            f.push_back(lpnlp);
            f.push_front(rpnlp);
        } else {
            let front = f.front_pt();
            let back = f.back_pt();
            if front != rpnlp && back != rpnlp {
                let split = f.find_split(f.pts[rpnlp]);
                f.split_back(split);
                f.push_front(rpnlp);
                if split > f.apex {
                    f.apex = split;
                }
            } else {
                let split = f.find_split(f.pts[lpnlp]);
                f.split_front(split);
                f.push_back(lpnlp);
                if split < f.apex {
                    f.apex = split;
                }
            }
        }
    }

    f.chain_from(dst_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn straight_through_square() {
        // 2x2 square, both endpoints inside — straight line.
        let poly = Polygon::new(vec![pt(0., 0.), pt(4., 0.), pt(4., 4.), pt(0., 4.)]).unwrap();
        let path = shortest_path(&poly, pt(1., 2.), pt(3., 2.)).unwrap();
        assert_eq!(path.points(), &[pt(1., 2.), pt(3., 2.)]);
    }

    #[test]
    fn rejects_endpoint_outside_polygon() {
        let poly = Polygon::new(vec![pt(0., 0.), pt(4., 0.), pt(4., 4.), pt(0., 4.)]).unwrap();
        assert_eq!(
            shortest_path(&poly, pt(-1., 2.), pt(3., 2.)),
            Err(PathError::SourceOutside)
        );
        assert_eq!(
            shortest_path(&poly, pt(1., 2.), pt(5., 2.)),
            Err(PathError::DestinationOutside)
        );
    }

    #[test]
    fn turns_through_l_shape_corner() {
        // L-shape (CCW): the upper-right square from (2,2)-(4,4) is bitten
        // out. Source (1, 3.5) is in the top-left rectangle, destination
        // (3.5, 1) is in the bottom-right rectangle. The straight line
        // crosses (2.25, 2.25) which is *outside* the polygon, so the
        // shortest path must detour around the inner-corner vertex (2, 2).
        //
        //   (0,4) +-----+ (2,4)
        //         |     |
        //         |     +------+ (4,2)
        //         |            |
        //   (0,0) +------------+ (4,0)
        let poly = Polygon::new(vec![
            pt(0., 0.),
            pt(4., 0.),
            pt(4., 2.),
            pt(2., 2.),
            pt(2., 4.),
            pt(0., 4.),
        ])
        .unwrap();
        let path = shortest_path(&poly, pt(1., 3.5), pt(3.5, 1.)).unwrap();
        let pts = path.points();
        assert_eq!(pts[0], pt(1., 3.5));
        assert_eq!(*pts.last().unwrap(), pt(3.5, 1.));
        assert!(
            pts.iter()
                .any(|p| (p.x - 2.0).abs() < 1e-9 && (p.y - 2.0).abs() < 1e-9),
            "expected inner corner waypoint, got {:?}",
            pts
        );
        assert!(pts.len() >= 3);
    }

    #[test]
    fn straight_line_grazing_corner_stays_two_points() {
        // Same L-shape, but the endpoints (1, 3) → (3, 1) lie on a line
        // that touches the inner corner (2, 2) without ever entering the
        // bite. The funnel should leave that as a single segment.
        let poly = Polygon::new(vec![
            pt(0., 0.),
            pt(4., 0.),
            pt(4., 2.),
            pt(2., 2.),
            pt(2., 4.),
            pt(0., 4.),
        ])
        .unwrap();
        let path = shortest_path(&poly, pt(1., 3.), pt(3., 1.)).unwrap();
        assert_eq!(path.points(), &[pt(1., 3.), pt(3., 1.)]);
    }

    #[test]
    fn returns_endpoints_only_in_same_triangle() {
        // Triangle polygon — both endpoints must land in the single
        // triangle, so the result is `[src, dst]`.
        let poly = Polygon::new(vec![pt(0., 0.), pt(10., 0.), pt(0., 10.)]).unwrap();
        let path = shortest_path(&poly, pt(2., 2.), pt(3., 3.)).unwrap();
        assert_eq!(path.points(), &[pt(2., 2.), pt(3., 3.)]);
    }
}
