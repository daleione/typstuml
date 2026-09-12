//! Geometry and anchor constraints, with no diagram semantics.
use crate::layout::geometry::Point;

pub(crate) struct NodeGeom {
    pub size: Point,
    /// Mid-x within the local frame. Used to anchor edge endpoints.
    pub mid_x: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Side {
    Top,
    Bot,
    Left,
    Right,
}

impl Side {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Bot => "bot",
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

pub(crate) fn bot_anchor(g: &NodeGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y + g.size.y)
}

pub(crate) fn top_anchor(g: &NodeGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y)
}

pub(crate) fn left_anchor(g: &NodeGeom, top_left: Point) -> Point {
    Point::new(top_left.x, top_left.y + g.size.y / 2.0)
}

pub(crate) fn right_anchor(g: &NodeGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.size.x, top_left.y + g.size.y / 2.0)
}

pub(crate) fn box_center(g: &NodeGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.size.x / 2.0, top_left.y + g.size.y / 2.0)
}

pub(crate) fn anchor_for_side(g: &NodeGeom, top_left: Point, side: Side) -> Point {
    match side {
        Side::Top => top_anchor(g, top_left),
        Side::Bot => bot_anchor(g, top_left),
        Side::Left => left_anchor(g, top_left),
        Side::Right => right_anchor(g, top_left),
    }
}
