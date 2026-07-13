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

/// Asymmetric extra space reserved on each side of a box, on top of its
/// symmetric halo. Used to reserve room for an ancestor cluster frame
/// (pad + label band) around a node that sits at the edge of that
/// cluster's row-span or rank-span, so the placer never packs a
/// stranger into space a frame will later occupy. Zero for every
/// element outside the cuca hierarchical layout path — see
/// `docs/cuca-architecture-layout-redesign.md` §3.2a.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Margin {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Margin {
    pub fn transpose(&self) -> Margin {
        Margin {
            left: self.top,
            right: self.bottom,
            top: self.left,
            bottom: self.right,
        }
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
    margin: Margin,
}

impl Position {
    pub fn new(middle: Point, size: Point, center: Point, halo: Point) -> Self {
        Position {
            middle,
            size,
            center,
            halo,
            margin: Margin::default(),
        }
    }

    /// `bbox(true)` reserves `margin` on top of the symmetric halo. Unlike
    /// the halo, margin is *not* split evenly around `middle` — a node
    /// with `margin.left > margin.right` gets more reserved space on its
    /// left than its right, so `bbox(true)` is no longer centered on
    /// `middle` when margins are asymmetric. `bbox(false)` (the real
    /// visual footprint) is unaffected.
    pub fn bbox(&self, with_halo: bool) -> (Point, Point) {
        let half = self.size.scale(0.5);
        let top_left = self.middle.sub(half);
        let bottom_right = self.middle.add(half);
        if with_halo {
            (
                Point::new(
                    top_left.x - self.halo.x / 2. - self.margin.left,
                    top_left.y - self.halo.y / 2. - self.margin.top,
                ),
                Point::new(
                    bottom_right.x + self.halo.x / 2. + self.margin.right,
                    bottom_right.y + self.halo.y / 2. + self.margin.bottom,
                ),
            )
        } else {
            (top_left, bottom_right)
        }
    }

    pub fn center(&self) -> Point {
        self.middle.add(self.center)
    }

    pub fn middle(&self) -> Point {
        self.middle
    }

    /// Total footprint including halo and margin (when `with_halo`).
    /// Always equal to `bbox(with_halo).1 - bbox(with_halo).0`.
    pub fn size(&self, with_halo: bool) -> Point {
        if with_halo {
            Point::new(
                self.size.x + self.halo.x + self.margin.left + self.margin.right,
                self.size.y + self.halo.y + self.margin.top + self.margin.bottom,
            )
        } else {
            self.size
        }
    }

    /// Accumulate extra reserved space; additive so multiple ancestor
    /// clusters can each contribute their own pad/label-band.
    pub fn add_margin(&mut self, m: Margin) {
        self.margin.left += m.left;
        self.margin.right += m.right;
        self.margin.top += m.top;
        self.margin.bottom += m.bottom;
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

    /// Set `middle.y` so that `bbox(true).0.y == y` (the reserved top
    /// edge, including margin, lands exactly at `y`).
    pub fn align_to_top(&mut self, y: f64) {
        self.middle.y = y + self.size.y / 2. + self.halo.y / 2. + self.margin.top;
    }

    /// Set `middle.x` so that `bbox(true).0.x == x`.
    pub fn align_to_left(&mut self, x: f64) {
        self.middle.x = x + self.size.x / 2. + self.halo.x / 2. + self.margin.left;
    }

    /// Set `middle.x` so that `bbox(true).1.x == x`.
    pub fn align_to_right(&mut self, x: f64) {
        self.middle.x = x - self.size.x / 2. - self.halo.x / 2. - self.margin.right;
    }

    pub fn set_x(&mut self, x: f64) {
        self.middle.x = x - self.center.x;
    }

    pub fn transpose(&mut self) {
        self.middle = self.middle.transpose();
        self.size = self.size.transpose();
        self.center = self.center.transpose();
        self.halo = self.halo.transpose();
        self.margin = self.margin.transpose();
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

