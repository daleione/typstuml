//! Orthogonal (Manhattan) edge router.
//!
//! Diagram-agnostic sparse visibility-grid A* router + rounded-corner
//! cubic conversion, built for cuca's desc-flavor (component /
//! deployment) diagrams — see
//! `docs/cuca-architecture-layout-redesign.md` §3.4. Not a Graphviz /
//! ELK port: the grid + A* + rounding are original, pure-Rust, and
//! tuned to this project's obstacle model (`pathplan::Box`).
//!
//! Pipeline: [`route`] finds a bend-minimizing orthogonal polyline
//! through a sparse grid built from obstacle boundaries; [`round::
//! simplify`] drops now-redundant collinear points; [`round::
//! to_rounded_cubics`] turns the polyline into the `(c1, c2, end)`
//! cubic list the blockcell painter already knows how to draw.

mod astar;
mod grid;
mod post;
mod round;

use crate::layout::geometry::Point;
use crate::layout::pathplan::Box as Obstacle;

pub use post::separate_overlapping;
pub use round::{simplify, to_rounded_cubics};

/// Midpoint of the longest straight segment in an orthogonal polyline
/// — where an edge label reads best (§3.8). The straight `start→end`
/// chord midpoint (what the painter uses for spline edges) can land
/// far from the actual path once it bends, so ortho-routed edges with
/// a label use this instead. `None` for a polyline with no segments
/// (fewer than 2 points).
pub fn longest_trunk_midpoint(points: &[Point]) -> Option<Point> {
    if points.len() < 2 {
        return None;
    }
    points
        .windows(2)
        .max_by(|a, b| {
            a[0].distance_to(a[1])
                .partial_cmp(&b[0].distance_to(b[1]))
                .unwrap()
        })
        .map(|seg| seg[0].add(seg[1]).scale(0.5))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    /// Outward-normal tangent (same convention as
    /// `codegen::cuca::route::side_tangent`) classified into the
    /// nearest grid direction.
    pub fn from_tangent(t: Point) -> Dir {
        if t.x.abs() >= t.y.abs() {
            if t.x >= 0.0 {
                Dir::Right
            } else {
                Dir::Left
            }
        } else if t.y >= 0.0 {
            Dir::Down
        } else {
            Dir::Up
        }
    }

    fn vector(self) -> Point {
        match self {
            Dir::Up => Point::new(0.0, -1.0),
            Dir::Down => Point::new(0.0, 1.0),
            Dir::Left => Point::new(-1.0, 0.0),
            Dir::Right => Point::new(1.0, 0.0),
        }
    }
}

pub struct RouteOpts {
    /// Grid clearance around every obstacle (ELK `spacing.edgeNode`).
    pub clearance: f64,
    /// Extra cost added per direction change, biasing the search
    /// toward long straight runs over many small jogs.
    pub bend_penalty: f64,
    /// Length of the forced launch/arrival stub perpendicular to each
    /// anchor face, so the route leaves/arrives square to the box
    /// without needing a direction constraint baked into the search.
    pub stub_len: f64,
}

impl Default for RouteOpts {
    fn default() -> Self {
        RouteOpts {
            clearance: 8.0,
            bend_penalty: 40.0,
            stub_len: 10.0,
        }
    }
}

/// Route `start -> end` through a sparse orthogonal visibility grid
/// built from `obstacles`, launching/arriving along `start_dir` /
/// `end_dir` (the anchor face's outward normal — see
/// `codegen::cuca::route::side_tangent`). Returns the polyline of bend
/// points (including `start` and `end`), or `None` if no path exists
/// (caller falls back to Manhattan / straight).
pub fn route(
    start: Point,
    start_dir: Dir,
    end: Point,
    end_dir: Dir,
    obstacles: &[Obstacle],
    opts: &RouteOpts,
) -> Option<Vec<Point>> {
    let launch = start.add(start_dir.vector().scale(opts.stub_len));
    let arrival = end.add(end_dir.vector().scale(opts.stub_len));

    let extra_points = [
        (start.x, start.y),
        (end.x, end.y),
        (launch.x, launch.y),
        (arrival.x, arrival.y),
    ];
    let g = grid::Grid::build(&extra_points, obstacles, opts.clearance);

    let start_i = grid::Grid::index_of(&g.xs, launch.x)?;
    let start_j = grid::Grid::index_of(&g.ys, launch.y)?;
    let end_i = grid::Grid::index_of(&g.xs, arrival.x)?;
    let end_j = grid::Grid::index_of(&g.ys, arrival.y)?;

    let path = astar::shortest_path(&g, start_i, start_j, end_i, end_j, opts.bend_penalty)?;
    let mut pts = astar::to_points(&g, &path);
    pts.insert(0, start);
    pts.push(end);
    Some(pts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_around_a_single_obstacle() {
        let ob = Obstacle::new(Point::new(40.0, -20.0), Point::new(60.0, 20.0));
        let opts = RouteOpts::default();
        let path = route(
            Point::new(0.0, 0.0),
            Dir::Right,
            Point::new(100.0, 0.0),
            Dir::Left,
            &[ob],
            &opts,
        )
        .expect("route found");
        assert_eq!(path.first().copied(), Some(Point::new(0.0, 0.0)));
        assert_eq!(path.last().copied(), Some(Point::new(100.0, 0.0)));
        // Must detour above or below the obstacle band [-20, 20].
        assert!(path.iter().any(|p| p.y > 20.0 || p.y < -20.0));
    }

    #[test]
    fn direct_route_with_no_obstacles_is_short() {
        let opts = RouteOpts::default();
        let path = route(
            Point::new(0.0, 0.0),
            Dir::Down,
            Point::new(0.0, 100.0),
            Dir::Up,
            &[],
            &opts,
        )
        .expect("route found");
        // No obstacles: launch/arrival stubs are the only bends, and
        // they lie on the direct line, so the whole thing collapses
        // close to a straight run.
        assert!(path.len() <= 4, "expected a short path, got {path:?}");
    }
}
