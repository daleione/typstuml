//! Port of `org.eclipse.elk.core.math.KVector` / `KVectorChain` —
//! just the operations the layered port uses.

/// A 2D vector / point (Java `KVector`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KVector {
    pub x: f64,
    pub y: f64,
}

impl KVector {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn add(mut self, other: KVector) -> Self {
        self.x += other.x;
        self.y += other.y;
        self
    }

    pub fn sub(mut self, other: KVector) -> Self {
        self.x -= other.x;
        self.y -= other.y;
        self
    }
}

/// Java `KVectorChain` — an ordered list of points (edge bend points).
pub type KVectorChain = Vec<KVector>;

/// Java `LMargin` / `LPadding` share this shape (they are distinct
/// classes upstream purely for type safety; the port keeps two type
/// aliases over one struct).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

pub type LMargin = Insets;
pub type LPadding = Insets;
