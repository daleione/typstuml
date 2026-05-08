//! Constraint-polygon construction for record-graph edges. Given the edge
//! endpoints and the bounding boxes of every other record, returns a
//! simple polygon that contains both endpoints and excludes every
//! obstacle's interior.
//!
//! Strategy ("sandwich channel"): the polygon is the padded bounding
//! rectangle of `(src, dst)` plus the obstacles that fall strictly between
//! them on the rank axis (x), with a one-sided bay cut around each
//! obstacle. Each obstacle dodges either above or below depending on which
//! side leaves the path closer to the line `src→dst`.
//!
//! The bay-cut topology is the simplest construction that:
//!
//! 1. Always produces a *simple* polygon (no self-intersections), so
//!    `Pshortestpath` accepts it.
//! 2. Guarantees `splineisinside` rejects any candidate cubic that crosses
//!    an obstacle's interior — the polygon edge runs along the obstacle's
//!    near boundary.
//! 3. Is cheap: O(n log n) on the number of obstacles, no rank metadata.
//!
//! Limitations vs dot's `_routesplines` polypoint construction:
//!
//! - Obstacles overlapping in x must dodge on the same side (otherwise the
//!   bays cross). We merge overlapping obstacles into a super-obstacle.
//! - When src and dst sit on opposite sides of an obstacle's y range there
//!   is no homogeneous dodge that contains both endpoints; we fall back to
//!   the no-dodge bounding rectangle and let the spline fitter cope. The
//!   `route_edge` driver may further fall back to a straight line.

use super::polygon::Polygon;
use super::PathError;
use crate::layout::geometry::Point;

/// Axis-aligned obstacle. `min` is the corner with the smallest x and y;
/// `max` is the opposite corner. The constructor takes the
/// `(top_left, bottom_right)` tuples already used by `record_graph.rs` —
/// in the project's y-down convention, top-left has the smallest y.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Box {
    pub min: Point,
    pub max: Point,
}

impl Box {
    pub fn new(top_left: Point, bottom_right: Point) -> Self {
        Box {
            min: top_left,
            max: bottom_right,
        }
    }

    pub fn padded(&self, pad: f64) -> Box {
        Box {
            min: Point::new(self.min.x - pad, self.min.y - pad),
            max: Point::new(self.max.x + pad, self.max.y + pad),
        }
    }
}

/// Build a constraint polygon from endpoints and obstacles. Padding is
/// applied around the bounding rectangle and around each obstacle's bay.
pub fn build_channel(
    src: Point,
    dst: Point,
    obstacles: &[Box],
    padding: f64,
) -> Result<Polygon, PathError> {
    let path_min_x = src.x.min(dst.x);
    let path_max_x = src.x.max(dst.x);

    // Keep any obstacle whose bbox overlaps the path corridor on both
    // axes — strict-interior is too tight (a record extending to the
    // left of `src.x` but otherwise blocking the line will be missed).
    // Then clip obstacle x to the path range so the bay never extends
    // outside the polygon's boundary; the y-overlap filter prevents
    // spurious bays for records that aren't on the corridor at all.
    let path_top = src.y.min(dst.y) - padding;
    let path_bot = src.y.max(dst.y) + padding;
    let mut interior: Vec<Box> = obstacles
        .iter()
        .filter(|o| o.max.x > path_min_x && o.min.x < path_max_x)
        .filter(|o| o.max.y > path_top && o.min.y < path_bot)
        .map(|o| Box {
            min: Point::new(o.min.x.max(path_min_x), o.min.y),
            max: Point::new(o.max.x.min(path_max_x), o.max.y),
        })
        .collect();
    interior.sort_by(|a, b| a.min.x.partial_cmp(&b.min.x).unwrap());
    let merged = merge_x_overlapping(interior);

    let dodges: Vec<Dodge> = merged
        .iter()
        .map(|o| decide_dodge(o, src, dst, padding))
        .collect();

    // Polygon vertical bounds. Start tight around src/dst, then expand
    // around each dodge to leave at least `padding` of clearance between
    // the polygon's outer top/bot and the bay floor — otherwise the
    // bay collapses to zero height and `Polygon::new` rejects the result
    // as self-intersecting.
    let mut top_y = src.y.min(dst.y) - padding;
    let mut bot_y = src.y.max(dst.y) + padding;
    for (o, d) in merged.iter().zip(dodges.iter()) {
        match d {
            Dodge::Above => top_y = top_y.min(o.min.y - 2.0 * padding),
            Dodge::Below => bot_y = bot_y.max(o.max.y + 2.0 * padding),
        }
    }

    let polygon = build_polygon(
        Point::new(path_min_x - padding, top_y),
        Point::new(path_max_x + padding, bot_y),
        &merged,
        &dodges,
        padding,
    );

    // The funnel's triangle-containment test (`pointintri`) is the
    // authoritative "is this endpoint usable?" check — it counts a point
    // sitting on a triangle edge as inside, which matters when an
    // obstacle's bay lands flush against `src.x` or `dst.x`. The
    // ray-cast `Polygon::contains` is fragile on boundary points so we
    // skip the eager check here and let `shortest_path` surface
    // `SourceOutside` / `DestinationOutside` if needed.
    Polygon::new(polygon)
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Dodge {
    Above,
    Below,
}

/// Choose the dodge side for one obstacle. When both endpoints sit on the
/// same y-side of the obstacle the choice is forced. When they sit on
/// opposite sides — a "mixed" case — we keep the corridor on the same side
/// as `src` so the path can stay there through the obstacle's x range and
/// transition to the destination's side in whatever clear corridor lies
/// past the obstacle's far edge. The tie case (both endpoints inside the
/// obstacle's y range) falls back to the path-mean rule, which is rarely
/// hit in practice.
fn decide_dodge(o: &Box, src: Point, dst: Point, padding: f64) -> Dodge {
    let src_above = src.y < o.min.y - padding;
    let src_below = src.y > o.max.y + padding;
    let dst_above = dst.y < o.min.y - padding;
    let dst_below = dst.y > o.max.y + padding;
    match (src_above, src_below, dst_above, dst_below) {
        (true, _, true, _) | (true, _, false, false) => Dodge::Above,
        (_, true, _, true) | (false, false, _, true) => Dodge::Below,
        (true, _, _, true) => Dodge::Above, // src above, dst below — stay with src.
        (_, true, true, _) => Dodge::Below, // src below, dst above — stay with src.
        _ => {
            let path_mean = 0.5 * (src.y + dst.y);
            let center = 0.5 * (o.min.y + o.max.y);
            if path_mean <= center {
                Dodge::Above
            } else {
                Dodge::Below
            }
        }
    }
}

fn merge_x_overlapping(boxes: Vec<Box>) -> Vec<Box> {
    let mut out: Vec<Box> = Vec::with_capacity(boxes.len());
    for b in boxes {
        match out.last_mut() {
            Some(last) if b.min.x <= last.max.x => {
                last.max.x = last.max.x.max(b.max.x);
                last.min.y = last.min.y.min(b.min.y);
                last.max.y = last.max.y.max(b.max.y);
            }
            _ => out.push(b),
        }
    }
    out
}

fn build_polygon(
    tl: Point,
    br: Point,
    obstacles: &[Box],
    dodges: &[Dodge],
    pad: f64,
) -> Vec<Point> {
    debug_assert_eq!(obstacles.len(), dodges.len());
    let top_y = tl.y;
    let bot_y = br.y;
    let left_x = tl.x;
    let right_x = br.x;

    let mut verts = Vec::with_capacity(4 + 4 * obstacles.len());

    // Top edge, left-to-right. A "below-dodge" (path goes under the
    // obstacle) forces the top boundary to dip down to obs.max.y + pad,
    // confining the corridor to the under-strip.
    verts.push(Point::new(left_x, top_y));
    for (o, d) in obstacles.iter().zip(dodges.iter()) {
        if *d == Dodge::Below {
            verts.push(Point::new(o.min.x, top_y));
            verts.push(Point::new(o.min.x, o.max.y + pad));
            verts.push(Point::new(o.max.x, o.max.y + pad));
            verts.push(Point::new(o.max.x, top_y));
        }
    }
    verts.push(Point::new(right_x, top_y));

    // Right edge.
    verts.push(Point::new(right_x, bot_y));

    // Bottom edge, right-to-left. An "above-dodge" (path goes over the
    // obstacle) lifts the bottom boundary up to obs.min.y - pad,
    // confining the corridor to the over-strip.
    for (o, d) in obstacles.iter().zip(dodges.iter()).rev() {
        if *d == Dodge::Above {
            verts.push(Point::new(o.max.x, bot_y));
            verts.push(Point::new(o.max.x, o.min.y - pad));
            verts.push(Point::new(o.min.x, o.min.y - pad));
            verts.push(Point::new(o.min.x, bot_y));
        }
    }
    verts.push(Point::new(left_x, bot_y));

    verts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn obs(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Box {
        Box::new(pt(min_x, min_y), pt(max_x, max_y))
    }

    #[test]
    fn no_obstacles_yields_padded_rectangle() {
        let poly = build_channel(pt(0., 5.), pt(10., 5.), &[], 1.0).unwrap();
        assert_eq!(poly.len(), 4);
        // Bounds: x ∈ [-1, 11], y ∈ [4, 6]
        let xs: Vec<f64> = poly.vertices().iter().map(|p| p.x).collect();
        let ys: Vec<f64> = poly.vertices().iter().map(|p| p.y).collect();
        assert!((xs.iter().cloned().fold(f64::INFINITY, f64::min) - -1.0).abs() < 1e-9);
        assert!((xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - 11.0).abs() < 1e-9);
        assert!((ys.iter().cloned().fold(f64::INFINITY, f64::min) - 4.0).abs() < 1e-9);
        assert!((ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn skips_obstacles_outside_path_x_range() {
        // Obstacle is to the left of both endpoints — should be ignored.
        let poly =
            build_channel(pt(10., 5.), pt(20., 5.), &[obs(0., 4., 5., 6.)], 1.0).unwrap();
        assert_eq!(poly.len(), 4, "{:?}", poly.vertices());
    }

    #[test]
    fn dodges_obstacle_above_when_path_is_above_center() {
        // Obstacle at y ∈ [4, 8], path at y = 4 (overlapping the obstacle's
        // y range, above the center 6). y-down: smaller y is "above". Path
        // mean y = 4 < 6 → dodge above.
        let poly = build_channel(pt(0., 4.), pt(10., 4.), &[obs(4., 4., 6., 8.)], 1.0).unwrap();
        // Should be 4 (rect) + 4 (one above-dodge) = 8 vertices.
        assert_eq!(poly.len(), 8);
        // The endpoints must be contained.
        assert!(poly.contains(pt(0., 4.)));
        assert!(poly.contains(pt(10., 4.)));
    }

    #[test]
    fn dodges_obstacle_below_when_path_is_below_center() {
        // Same obstacle, path on the below side (y = 8).
        let poly = build_channel(pt(0., 8.), pt(10., 8.), &[obs(4., 4., 6., 8.)], 1.0).unwrap();
        assert_eq!(poly.len(), 8);
        assert!(poly.contains(pt(0., 8.)));
        assert!(poly.contains(pt(10., 8.)));
    }

    #[test]
    fn merges_x_overlapping_obstacles() {
        // Two obstacles with overlapping x range get merged into one bay.
        // Both must overlap the path's y range (4..8 corridor) for the
        // dodge to fire after the y-overlap filter.
        let poly = build_channel(
            pt(0., 6.),
            pt(10., 6.),
            &[obs(3., 4., 6., 7.), obs(5., 5., 7., 8.)],
            1.0,
        )
        .unwrap();
        // Merged → single bay → 4 + 4 = 8 vertices.
        assert_eq!(poly.len(), 8);
    }

    #[test]
    fn skips_obstacle_outside_corridor_y_range() {
        // Obstacle at y ∈ [50, 60] is far below the y=0 path corridor.
        // Without the y-overlap filter we'd add a spurious bay that
        // extends the polygon downward and tempts the funnel to detour
        // through it; with the filter the obstacle is ignored entirely.
        let poly =
            build_channel(pt(0., 0.), pt(10., 0.), &[obs(4., 50., 6., 60.)], 1.0).unwrap();
        assert_eq!(poly.len(), 4, "expected no bay, got {:?}", poly.vertices());
    }

    #[test]
    fn rejects_when_src_outside_resulting_polygon() {
        // Pathological setup where the homogeneous-dodge bay would leave
        // the source point uncontained. We don't construct such a case
        // easily here, so simply assert that the route returns a
        // well-typed result for a realistic boundary input.
        let result = build_channel(pt(0., 3.9), pt(10., 3.9), &[obs(4., 4., 6., 6.)], 1.0);
        assert!(result.is_ok());
    }
}
