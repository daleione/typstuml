//! Flow shape metrics and anchor capabilities; paired with shape-flow.typ.
use crate::codegen::graph::{geom::NodeGeom, routing::AnchorConstraints};
use crate::ir::{FlowNode, FlowShape};
use crate::layout::geometry::Point;

pub(super) fn measured(w: f64, h: f64) -> NodeGeom {
    NodeGeom {
        size: Point::new(w, h),
        mid_x: w / 2.0,
    }
}

pub(super) fn estimate(node: &FlowNode) -> NodeGeom {
    let mut w = node
        .label
        .split('\n')
        .map(|s| {
            s.chars()
                .map(|c| if c.is_ascii() { 5.5 } else { 10.0 })
                .sum::<f64>()
        })
        .fold(0.0, f64::max)
        + 24.0;
    let mut h = (node.label.split('\n').count() as f64 * 12.0 + 16.0).max(28.0);
    match node.shape {
        FlowShape::Diamond => {
            w *= 2.0;
            h *= 2.0;
        }
        FlowShape::Circle => {
            w = w.hypot(h);
            h = w;
        }
        FlowShape::Stadium => w += h,
        FlowShape::Cylinder => h += 12.0,
        FlowShape::Asymmetric => w += 24.0,
        _ => {}
    }
    measured(w, h)
}

pub(super) fn estimate_label(label: &str) -> Point {
    Point::new(
        label
            .lines()
            .map(|s| s.chars().count() as f64 * 5.0)
            .fold(0.0, f64::max)
            + 4.0,
        label.lines().count().max(1) as f64 * 10.0 + 4.0,
    )
}

pub(super) fn constraints(shape: FlowShape) -> AnchorConstraints {
    AnchorConstraints {
        fixed: shape != FlowShape::Rect,
        left_inset: if shape == FlowShape::Asymmetric {
            12.0
        } else {
            0.0
        },
        prefers_spline: true,
    }
}
