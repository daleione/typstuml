use crate::layout::geometry::Point;

/// Place labels near the actual route, avoiding nodes and labels already placed.
/// The explicit center also lets the Typst painter include label bounds in its canvas.
pub(crate) fn place_label(
    start: Point,
    segments: &[(Point, Point, Point)],
    size: Point,
    occupied: &mut Vec<(Point, Point)>,
) -> Point {
    let mut previous = start;
    let mut best = (0.0, start);
    for &(c1, c2, end) in segments {
        let distance = (end.x - previous.x).hypot(end.y - previous.y);
        if distance >= best.0 {
            best = (
                distance,
                Point::new(
                    (previous.x + 3.0 * c1.x + 3.0 * c2.x + end.x) / 8.0,
                    (previous.y + 3.0 * c1.y + 3.0 * c2.y + end.y) / 8.0,
                ),
            );
        }
        previous = end;
    }
    let center = best.1;
    let bounds = |p: Point| {
        (
            Point::new(p.x - size.x / 2.0 - 2.0, p.y - size.y / 2.0 - 2.0),
            Point::new(p.x + size.x / 2.0 + 2.0, p.y + size.y / 2.0 + 2.0),
        )
    };
    let clear = |b: (Point, Point), occupied: &[(Point, Point)]| {
        !occupied
            .iter()
            .any(|&(lo, hi)| b.0.x < hi.x && b.1.x > lo.x && b.0.y < hi.y && b.1.y > lo.y)
    };
    for ring in 0..=32 {
        let offset = ring as f64 * 12.0;
        for (dx, dy) in [(0.0, offset), (0.0, -offset), (offset, 0.0), (-offset, 0.0)] {
            let p = Point::new(center.x + dx, center.y + dy);
            let b = bounds(p);
            if clear(b, occupied) {
                occupied.push(b);
                return p;
            }
        }
    }
    // Dense graph: put the label beyond the occupied extent instead of overlapping.
    let right = occupied.iter().map(|(_, hi)| hi.x).fold(center.x, f64::max);
    let p = Point::new(right + size.x / 2.0 + 12.0, center.y);
    occupied.push(bounds(p));
    p
}
