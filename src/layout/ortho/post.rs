//! Batch post-pass: fan apart parallel orthogonal trunk segments
//! across *all* of a diagram's routed edges so they don't visually
//! overlap (ELK's `separateOverlappingEdges`, §3.5.2).
//!
//! Must run after every edge's polyline is known (hence "batch" — this
//! is not a per-edge pass like `simplify`/`to_rounded_cubics`), and
//! before rounding, since it moves the same raw axis-aligned points
//! those functions consume.

use crate::layout::geometry::Point;

/// A segment eligible to move: `route`/`k` locate it (`points[k]` to
/// `points[k + 1]` inside `routes[route]`); `coord` is its position on
/// the shared axis (x for a vertical segment, y for horizontal);
/// `span` is its extent on the other axis; `length` breaks ties when
/// picking which segment in a group anchors the rest.
#[derive(Clone, Copy)]
struct SegRef {
    route: usize,
    k: usize,
    vertical: bool,
    coord: f64,
    span: (f64, f64),
    length: f64,
}

/// Fan apart parallel trunk segments so they don't overlap. Only
/// segments whose *both* endpoints are interior bend points (index
/// `1..=len-2` segments, i.e. excluding the first and last segment of
/// each polyline) are eligible — those touch the true anchor point on
/// a box face and must stay exactly where the router put them. Movable
/// segments always sit between two perpendicular neighbors (routes are
/// simplified, alternating-axis polylines), so shifting one along its
/// own perpendicular axis only changes its neighbors' *length*, never
/// their direction — every route stays axis-aligned.
///
/// `min_gap` is both the grouping tolerance (segments within `min_gap`
/// of each other, same axis, overlapping span, count as "the same
/// trunk") and the fan-out spacing applied once grouped.
pub fn separate_overlapping(routes: &mut [Vec<Point>], min_gap: f64) {
    let mut segs: Vec<SegRef> = Vec::new();
    for (ri, pts) in routes.iter().enumerate() {
        if pts.len() < 4 {
            continue; // need >= 3 segments for an eligible interior one
        }
        let n = pts.len() - 1; // segment count
        for k in 1..=n.saturating_sub(2) {
            let a = pts[k];
            let b = pts[k + 1];
            let vertical = (a.x - b.x).abs() < 1e-6;
            let horizontal = (a.y - b.y).abs() < 1e-6;
            if vertical {
                segs.push(SegRef {
                    route: ri,
                    k,
                    vertical: true,
                    coord: a.x,
                    span: (a.y.min(b.y), a.y.max(b.y)),
                    length: (a.y - b.y).abs(),
                });
            } else if horizontal {
                segs.push(SegRef {
                    route: ri,
                    k,
                    vertical: false,
                    coord: a.y,
                    span: (a.x.min(b.x), a.x.max(b.x)),
                    length: (a.x - b.x).abs(),
                });
            }
            // Non-axis-aligned "segments" shouldn't occur post-simplify;
            // silently skip rather than panic if one slips through.
        }
    }

    let n_segs = segs.len();
    let mut group_id = vec![usize::MAX; n_segs];
    let mut next_group = 0usize;
    for i in 0..n_segs {
        for j in (i + 1)..n_segs {
            if segs[i].vertical != segs[j].vertical || segs[i].route == segs[j].route {
                continue;
            }
            if (segs[i].coord - segs[j].coord).abs() > min_gap {
                continue;
            }
            let overlap = segs[i].span.1.min(segs[j].span.1) - segs[i].span.0.max(segs[j].span.0);
            if overlap <= 0.0 {
                continue;
            }
            match (group_id[i], group_id[j]) {
                (usize::MAX, usize::MAX) => {
                    group_id[i] = next_group;
                    group_id[j] = next_group;
                    next_group += 1;
                }
                (g, usize::MAX) => group_id[j] = g,
                (usize::MAX, g) => group_id[i] = g,
                (gi, gj) if gi != gj => {
                    for gid in group_id.iter_mut() {
                        if *gid == gj {
                            *gid = gi;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, &g) in group_id.iter().enumerate() {
        if g != usize::MAX {
            groups.entry(g).or_default().push(i);
        }
    }

    for members in groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        let mut sorted = members;
        sorted.sort_by(|&a, &b| segs[b].length.partial_cmp(&segs[a].length).unwrap());
        let anchor_coord = segs[sorted[0]].coord;
        for (offset_idx, &si) in sorted[1..].iter().enumerate() {
            let step = offset_idx / 2 + 1;
            let sign = if offset_idx % 2 == 0 { 1.0 } else { -1.0 };
            let target = anchor_coord + sign * (step as f64) * min_gap;
            let delta = target - segs[si].coord;
            let seg = segs[si];
            let pts = &mut routes[seg.route];
            if seg.vertical {
                pts[seg.k].x += delta;
                pts[seg.k + 1].x += delta;
            } else {
                pts[seg.k].y += delta;
                pts[seg.k + 1].y += delta;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_vertical_trunks_fan_apart() {
        // Two routes whose middle (interior) segment runs vertically
        // at the same x — must separate by at least min_gap.
        let mut routes = vec![
            vec![
                Point::new(0.0, 0.0),
                Point::new(50.0, 0.0),
                Point::new(50.0, 100.0),
                Point::new(100.0, 100.0),
            ],
            vec![
                Point::new(0.0, 20.0),
                Point::new(50.0, 20.0),
                Point::new(50.0, 120.0),
                Point::new(100.0, 120.0),
            ],
        ];
        separate_overlapping(&mut routes, 10.0);
        let x0 = routes[0][1].x;
        let x1 = routes[1][1].x;
        assert!((x0 - x1).abs() >= 10.0 - 1e-6, "got x0={x0} x1={x1}");
        // Interior segment endpoints still share x within each route
        // (still vertical) and the dock (first/last) segments' anchor
        // points never moved.
        assert_eq!(routes[0][1].x, routes[0][2].x);
        assert_eq!(routes[0][0], Point::new(0.0, 0.0));
        assert_eq!(routes[0][3], Point::new(100.0, 100.0));
    }

    #[test]
    fn short_routes_are_left_alone() {
        // A single-segment (2-point) and simple Z-route with no
        // interior-interior segment eligible: nothing to move.
        let mut routes = vec![
            vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)],
        ];
        let before = routes.clone();
        separate_overlapping(&mut routes, 10.0);
        assert_eq!(routes, before);
    }

    #[test]
    fn segments_of_the_same_route_never_group_with_each_other() {
        // Two interior vertical segments in the *same* route, close
        // enough in x (2pt apart, well under the 10pt gap) and
        // overlapping in span, that they'd wrongly fan apart if the
        // same-route exclusion were missing.
        let mut routes = vec![vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 60.0),
            Point::new(52.0, 60.0),
            Point::new(52.0, 0.0),
            Point::new(100.0, 0.0),
        ]];
        let before = routes.clone();
        separate_overlapping(&mut routes, 10.0);
        assert_eq!(routes, before);
    }
}
