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

    pub fn set_size(&mut self, size: Point) {
        self.size = size;
    }

    pub fn set_new_center_point(&mut self, center: Point) {
        self.center = center;
        debug_assert!(center.x.abs() <= self.size.x / 2.);
        debug_assert!(center.y.abs() <= self.size.y / 2.);
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

pub fn median(vec: &[f64]) -> f64 {
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

/// Closed-rectangle overlap: boxes whose edges just touch are treated as
/// intersecting. The placer keeps a small EPSILON gap between rank
/// neighbours, so this only matters as a tightness check, not as a routine
/// false-positive trigger.
pub fn do_boxes_intersect(p1: (Point, Point), p2: (Point, Point)) -> bool {
    let overlap_x = p2.0.x <= p1.1.x && p1.0.x <= p2.1.x;
    let overlap_y = p2.0.y <= p1.1.y && p1.0.y <= p2.1.y;
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

