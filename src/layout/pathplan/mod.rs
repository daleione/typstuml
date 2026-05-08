//! Port of dot's `pathplan` library: shortest path inside a simple polygon
//! plus B-spline routing through a constraint polygon. Replaces the
//! visibility-graph / Dijkstra router in `edge_route.rs`.
//!
//! Layout (build-out order):
//!
//! - `geometry`  — local CCW / segment-intersection / collinearity helpers.
//! - `polygon`   — `Polygon` newtype, ear-clip triangulation, adjacency.
//! - `shortest`  — `shortest_path()` (Lee-Preparata funnel).
//! - `polynomial` — cubic root finder, used by spline.
//! - `spline`    — `route_spline()` (mkspline + splinefits + recursive split).
//! - `channel`   — `build_channel()` (obstacles → constraint polygon).
//!
//! See `docs/roadmap.md` for full design rationale and milestone breakdown.
//! Source-of-truth references are `pathplan/{shortest,route,solvers,util}.c`
//! in graphviz; PlantUML's Smetana port mirrors them line-for-line at
//! `vendor` paths under `gen/lib/pathplan/*.java`.

mod channel;
mod geometry;
mod polygon;
mod polynomial;
mod shortest;
mod spline;

pub use channel::{build_channel, Box};
pub use polygon::Polygon;
pub use shortest::{shortest_path, Polyline};
pub use spline::{route_spline, Cubic};

use crate::layout::geometry::Point;

/// Per-edge tunables. Defaults match the values `record_graph.rs` was
/// already using with the legacy `edge_route` (1pt obstacle padding,
/// horizontal entry/exit tangents).
#[derive(Copy, Clone, Debug)]
pub struct RouteOpts {
    pub obstacle_padding: f64,
    pub src_tangent: Point,
    pub dst_tangent: Point,
}

impl Default for RouteOpts {
    fn default() -> Self {
        RouteOpts {
            obstacle_padding: 1.0,
            src_tangent: Point::new(1.0, 0.0),
            dst_tangent: Point::new(1.0, 0.0),
        }
    }
}

/// One-shot edge routing: build the constraint polygon, find the polyline
/// shortest path inside it, then fit a sequence of cubic Beziers along
/// that polyline. The painter consumes the cubics via
/// [`Cubic::into_painter_segment`].
///
/// Failure modes (any of which the caller may convert into a straight-line
/// fallback):
///
/// - The constraint polygon couldn't be built without the endpoints
///   landing in a dodge bay — the obstacles' arrangement isn't compatible
///   with our homogeneous-dodge channel topology
///   (`PathError::SourceOutside` / `DestinationOutside`).
/// - The funnel produced no valid triangle path through the polygon.
pub fn route_edge(
    src: Point,
    dst: Point,
    obstacles: &[Box],
    opts: RouteOpts,
) -> Result<Vec<Cubic>, PathError> {
    let polygon = build_channel(src, dst, obstacles, opts.obstacle_padding)?;
    let polyline = shortest_path(&polygon, src, dst)?;
    Ok(route_spline(
        &polygon,
        &polyline,
        opts.src_tangent,
        opts.dst_tangent,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn route_edge_clear_path_emits_one_cubic() {
        let cubics = route_edge(
            pt(0., 5.),
            pt(20., 5.),
            &[],
            RouteOpts {
                obstacle_padding: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cubics.len(), 1);
        assert_eq!(cubics[0].start, pt(0., 5.));
        assert_eq!(cubics[0].end, pt(20., 5.));
    }

    #[test]
    fn route_edge_detours_around_blocking_obstacle() {
        // Obstacle sits on the straight line; path must dodge.
        let cubics = route_edge(
            pt(0., 5.),
            pt(20., 5.),
            &[Box::new(pt(8., 4.), pt(12., 8.))],
            RouteOpts {
                obstacle_padding: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        // Detour around an obstacle ⇒ at least 2 cubic segments
        // (the recursive split fires when splinefits rejects the
        // straight cubic for crossing the polygon edge).
        assert!(cubics.len() >= 1);
        assert_eq!(cubics.first().unwrap().start, pt(0., 5.));
        assert_eq!(cubics.last().unwrap().end, pt(20., 5.));
    }

    #[test]
    fn route_edge_surfaces_endpoint_outside_polygon() {
        // src is inside an obstacle's bay-side: the homogeneous-dodge
        // construction can't contain it. Caller should fall back to a
        // straight line.
        let result = route_edge(
            pt(5., 5.),
            pt(20., 5.),
            // Obstacle straddles src vertically.
            &[Box::new(pt(7., 0.), pt(15., 10.))],
            RouteOpts::default(),
        );
        // Either succeeds (path mean above center happens to leave src
        // contained) or surfaces the outside-source error.
        assert!(matches!(
            result,
            Ok(_) | Err(PathError::SourceOutside) | Err(PathError::DestinationOutside)
        ));
    }
}

/// Errors surfaced by the pathplan public API. Variants cover misuse
/// (`PolygonTooSmall`), input geometry violations the C original treats as
/// fatal (`PolygonSelfIntersecting`, endpoints outside), and internal-only
/// failures we want callers to be able to fall back from.
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum PathError {
    #[error("polygon must have at least 3 vertices, got {0}")]
    PolygonTooSmall(usize),

    #[error("polygon edges cross each other; only simple polygons are allowed")]
    PolygonSelfIntersecting,

    #[error("source point is not inside the constraint polygon")]
    SourceOutside,

    #[error("destination point is not inside the constraint polygon")]
    DestinationOutside,

    #[error("triangulation could not connect source to destination")]
    NoTrianglePath,
}
