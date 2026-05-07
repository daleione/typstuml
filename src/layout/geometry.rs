//! 2D points, sized positions, and the small helpers the Sugiyama placer
//! and bezier router need. All coordinates are in Typst pt — there is no
//! separate "layout unit" anymore.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn zero() -> Point {
        Point { x: 0., y: 0. }
    }

    pub fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    pub fn splat(s: f64) -> Point {
        Point::new(s, s)
    }

    pub fn neg(&self) -> Point {
        Point::new(-self.x, -self.y)
    }

    pub fn add(&self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }

    pub fn sub(&self, other: Point) -> Point {
        self.add(other.neg())
    }

    pub fn scale(&self, s: f64) -> Point {
        Point::new(self.x * s, self.y * s)
    }

    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn distance_to(&self, other: Point) -> f64 {
        self.sub(other).length()
    }

    pub fn transpose(&self) -> Point {
        Point::new(self.y, self.x)
    }

    pub fn rotate(&self, angle: f64) -> Point {
        let (s, c) = (angle.sin(), angle.cos());
        Point::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "(x: {:.3}, y: {:.3})", self.x, self.y)
    }
}

/// A node's box on the canvas: an absolute centerpoint, an inner size, and
/// a symmetric halo (the gap around the box reserved for spacing). Originally
/// upstream supported a `center` delta separate from `middle` to anchor edges
/// at one side of label nodes — we don't render labels, so middle == center
/// in practice, but the field is preserved because the placer still touches
/// it via `set_new_center_point` for connector placement.
#[derive(Debug, Clone, Copy)]
pub struct Position {
    middle: Point,
    size: Point,
    center: Point,
    halo: Point,
}

impl Position {
    pub fn new(middle: Point, size: Point, center: Point, halo: Point) -> Self {
        Position {
            middle,
            size,
            center,
            halo,
        }
    }

    pub fn bbox(&self, with_halo: bool) -> (Point, Point) {
        let size = self.size(with_halo);
        let top_left = self.middle.sub(size.scale(0.5));
        (top_left, top_left.add(size))
    }

    pub fn center(&self) -> Point {
        self.middle.add(self.center)
    }

    pub fn middle(&self) -> Point {
        self.middle
    }

    pub fn size(&self, with_halo: bool) -> Point {
        if with_halo {
            self.size.add(self.halo)
        } else {
            self.size
        }
    }

    pub fn left(&self, with_halo: bool) -> f64 {
        self.bbox(with_halo).0.x
    }

    pub fn right(&self, with_halo: bool) -> f64 {
        self.bbox(with_halo).1.x
    }

    pub fn distance_to_left(&self, with_halo: bool) -> f64 {
        self.center().x - self.left(with_halo)
    }

    pub fn distance_to_right(&self, with_halo: bool) -> f64 {
        self.right(with_halo) - self.center().x
    }

    pub fn in_x_range(&self, range: (f64, f64), with_halo: bool) -> bool {
        self.left(with_halo) >= range.0 && self.right(with_halo) <= range.1
    }

    pub fn set_size(&mut self, size: Point) {
        self.size = size;
    }

    pub fn set_new_center_point(&mut self, center: Point) {
        self.center = center;
        debug_assert!(center.x.abs() < self.size.x);
        debug_assert!(center.y.abs() < self.size.y);
    }

    pub fn move_to(&mut self, p: Point) {
        let delta = p.sub(self.center());
        self.middle = self.middle.add(delta);
    }

    pub fn translate(&mut self, d: Point) {
        self.middle = self.middle.add(d);
    }

    pub fn align_to_top(&mut self, y: f64) {
        self.middle.y = y + self.size.y / 2. + self.halo.y / 2.;
    }

    pub fn align_to_left(&mut self, x: f64) {
        self.middle.x = x + self.size.x / 2. + self.halo.x / 2.;
    }

    pub fn align_to_right(&mut self, x: f64) {
        self.middle.x = x - self.size.x / 2. - self.halo.x / 2.;
    }

    pub fn set_x(&mut self, x: f64) {
        self.middle.x = x - self.center.x;
    }

    pub fn set_y(&mut self, y: f64) {
        self.middle.y = y - self.center.y;
    }

    pub fn transpose(&mut self) {
        self.middle = self.middle.transpose();
        self.size = self.size.transpose();
        self.center = self.center.transpose();
        self.halo = self.halo.transpose();
    }
}

// ---------------------------------------------------------------------------
// Helpers used by the placer (median splitting, overlap test) and by the
// bezier router (vector math, box-edge intersection, passthrough control
// point).

pub fn weighted_median(vec: &[f64]) -> f64 {
    assert!(!vec.is_empty(), "array can't be empty");
    let mut vec = vec.to_vec();
    vec.sort_by(|a, b| a.partial_cmp(b).unwrap());
    match vec.len() {
        1 => vec[0],
        2 => (vec[0] + vec[1]) / 2.,
        n if n % 2 == 1 => vec[n / 2],
        n => (vec[n / 2] + vec[n / 2 - 1]) / 2.,
    }
}

pub fn in_range(range: (f64, f64), x: f64) -> bool {
    x >= range.0 && x <= range.1
}

fn approx_eq(x: f64, y: f64) -> bool {
    let abs_diff = (x - y).abs();
    if abs_diff < f64::EPSILON {
        return true;
    }
    abs_diff / x.abs().max(y.abs()).max(f64::EPSILON) < f64::EPSILON
}

fn le_approx(x: f64, y: f64) -> bool {
    x < y || approx_eq(x, y)
}

pub fn do_boxes_intersect(p1: (Point, Point), p2: (Point, Point)) -> bool {
    let overlap_x = le_approx(p2.0.x, p1.1.x) && le_approx(p1.0.x, p2.1.x);
    let overlap_y = le_approx(p2.0.y, p1.1.y) && le_approx(p1.0.y, p2.1.y);
    overlap_x && overlap_y
}

pub fn segment_rect_intersection(seg: (Point, Point), rect: (Point, Point)) -> bool {
    debug_assert!(rect.0.x <= rect.1.x);
    debug_assert!(rect.0.y <= rect.1.y);

    if seg.0.x == seg.1.x {
        return seg.1.x >= rect.0.x && seg.1.x <= rect.1.x;
    }
    let xs_outside =
        (seg.0.x < rect.0.x && seg.1.x < rect.0.x) || (seg.0.x > rect.1.x && seg.1.x > rect.1.x);
    let ys_outside =
        (seg.0.y < rect.0.y && seg.1.y < rect.0.y) || (seg.0.y > rect.1.y && seg.1.y > rect.1.y);
    if xs_outside || ys_outside {
        return false;
    }

    let dx = seg.1.x - seg.0.x;
    let dy = seg.1.y - seg.0.y;
    let a = dy / dx;
    let b = seg.0.y - a * seg.0.x;
    let y0 = a * rect.0.x + b;
    let y1 = a * rect.1.x + b;
    !((y0 < rect.0.y && y1 < rect.0.y) || (y0 > rect.1.y && y1 > rect.1.y))
}

// ---------------------------------------------------------------------------
// Edge routing geometry.

fn normalize_scale(v: Point, s: f64) -> Point {
    let len = v.length();
    debug_assert!(len > 0., "can't normalize the zero vector");
    v.scale(s / len)
}

/// A segment of length `s` aimed from `from` toward `to`. If they coincide,
/// fall back to a unit horizontal vector to avoid NaN.
fn segment_toward(from: Point, to: Point, s: f64) -> (Point, Point) {
    if from == to {
        return (from, Point::new(from.x + s, from.y));
    }
    let dir = normalize_scale(to.sub(from), s);
    (from, dir.add(from))
}

fn interpolate(v0: Point, v1: Point, w: f64) -> Point {
    v0.scale(w).add(v1.scale(1. - w))
}

/// Where does an edge coming from `from` intersect a box of size `size`
/// centered at `loc`? Returns the intersection plus a control-point pulled
/// outward by `force` along the edge direction — the format the bezier
/// router consumes.
pub fn box_edge_intersection(loc: Point, size: Point, from: Point, force: f64) -> (Point, Point) {
    let mut loc = loc;
    let mut size = size;

    // If the source clearly lies past one half of the box, focus on the
    // closer half so the connection lands on the visible side rather than
    // the far edge.
    if from.x > loc.x + size.x / 2. {
        size.x /= 2.;
        loc.x += size.x / 2.;
    } else if from.x < loc.x - size.x / 2. {
        size.x /= 2.;
        loc.x -= size.x / 2.;
    }

    let dx = loc.x - from.x;
    let dy = loc.y - from.y;
    let half_x = size.x / 2.;
    let half_y = size.y / 2.;

    if dx == 0. {
        let y = if dy > 0. {
            loc.y - half_y
        } else {
            loc.y + half_y
        };
        return segment_toward(Point::new(loc.x, y), from, force);
    }

    let slope = dy / dx;
    let gain_y = half_x * slope;

    if gain_y.abs() < half_y {
        let (bx, gy) = if dx > 0. {
            (-half_x, -gain_y)
        } else {
            (half_x, gain_y)
        };
        return segment_toward(Point::new(loc.x + bx, loc.y + gy), from, force);
    }

    let gain_x = half_y / slope;
    let (by, gx) = if dy > 0. {
        (-half_y, -gain_x)
    } else {
        (half_y, gain_x)
    };
    segment_toward(Point::new(loc.x + gx, loc.y + by), from, force)
}

/// Bezier control-point for an edge passing through an invisible connector
/// node at `center`, coming from `from` and heading to `to`.
pub fn passthrough_control_point(
    center: Point,
    from: Point,
    to: Point,
    force: f64,
) -> (Point, Point) {
    let ar = center.sub(from);
    let rb = to.sub(center);
    let a_out = normalize_scale(ar.neg(), force);
    let b_out = normalize_scale(rb.neg(), force);

    // Self-loop case: the two outgoing tangents cancel. Twist 90° to keep
    // the curve away from the source.
    if a_out.add(b_out).length() < 1. {
        let edge = a_out.rotate(90_f64.to_radians());
        return (center, edge.add(center));
    }

    // Mix the two tangents in inverse proportion to segment length, so a
    // close-by source dominates and the curve doesn't overshoot.
    let mut a_ratio = ar.length() / (ar.length() + rb.length());
    if center.x == to.x || center.y == to.y {
        a_ratio = 1.;
    } else if center.x == from.x || center.y == from.y {
        a_ratio = 0.;
    }
    let res = interpolate(a_out, b_out, 1. - a_ratio);
    (center, res.add(center))
}
