//! Flowchart-to-painter wire encoding. No PlantUML text or semantic types.
use crate::codegen::{
    common::escape_string,
    graph::{
        compound::LayoutResult,
        geom::{NodeGeom, Side},
        routing::{RouteEdge, RoutedEdge},
    },
};
use crate::ir::{
    FlowEdge, FlowNode, FlowShape, FlowchartDiagram, LayoutDirection, LineStyle, LineWeight,
};
use std::fmt::Write as _;

pub(super) fn plain_text(value: &str) -> String {
    value
        .split('\n')
        .map(|s| format!("#text(\"{}\")", escape_string(s)))
        .collect::<Vec<_>>()
        .join("#linebreak()")
}

pub(super) fn node_spec(out: &mut String, node: &FlowNode) {
    let kind = match node.shape {
        FlowShape::Rect => "flow-rect",
        FlowShape::Rounded => "flow-rounded",
        FlowShape::Stadium => "flow-stadium",
        FlowShape::Diamond => "flow-diamond",
        FlowShape::Circle => "flow-circle",
        FlowShape::Cylinder => "flow-cylinder",
        FlowShape::Asymmetric => "flow-asymmetric",
    };
    let _ = write!(out, "kind: \"{kind}\", name: [{}]", plain_text(&node.label));
}

pub(super) fn diagram(
    out: &mut String,
    diag: &FlowchartDiagram,
    geoms: &[NodeGeom],
    layout: &LayoutResult,
    edges: &[&FlowEdge],
    endpoints: &[RouteEdge],
    routes: &[RoutedEdge],
) {
    out.push_str("#flowchart-layout(\n");
    if diag.direction == LayoutDirection::LeftToRight {
        out.push_str("  direction: \"lr\",\n");
    }
    out.push_str("  nodes: (\n");
    for ((node, geom), p) in diag.nodes.iter().zip(geoms).zip(&layout.top_lefts) {
        let _ = write!(
            out,
            "    (x: {:.2}pt, y: {:.2}pt, width: {:.2}pt, height: {:.2}pt, ",
            p.x, p.y, geom.size.x, geom.size.y
        );
        node_spec(out, node);
        out.push_str("),\n");
    }
    out.push_str("  ),\n");
    if !diag.subgraphs.is_empty() {
        out.push_str("  subgraphs: (\n");
        for (group, bbox) in diag.subgraphs.iter().zip(&layout.container_bboxes) {
            let Some((lo, hi)) = bbox else { continue };
            let _ = writeln!(out, "    (x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, kind: \"package\", label: [{}]),",
                lo.x, lo.y, hi.x-lo.x, hi.y-lo.y, plain_text(&group.label));
        }
        out.push_str("  ),\n");
    }
    out.push_str("  edges: (\n");
    for ((edge, endpoints), route) in edges.iter().zip(endpoints).zip(routes) {
        let style = match edge.style {
            LineStyle::Solid => "solid",
            LineStyle::Dashed => "dashed",
            LineStyle::Dotted => "dotted",
        };
        let head = if edge.arrow { "arrow-open" } else { "none" };
        let _ = write!(out, "    (from: {}, to: {}, head-from: \"none\", head-to: \"{head}\", style: \"{style}\", from-side: \"{}\", to-side: \"{}\"",
            endpoints.src_idx, endpoints.dst_idx, route.sides.0.keyword(), route.sides.1.keyword());
        for (prefix, side, coord) in [
            ("from", route.sides.0, route.from_override),
            ("to", route.sides.1, route.to_override),
        ] {
            if let Some(c) = coord {
                let axis = if matches!(side, Side::Left | Side::Right) {
                    "y"
                } else {
                    "x"
                };
                let _ = write!(out, ", {prefix}-{axis}: {c:.2}pt");
            }
        }
        if edge.weight == LineWeight::Thick {
            out.push_str(", weight: 2");
        }
        if let Some(label) = &edge.label {
            let _ = write!(out, ", label: [{}]", plain_text(label));
        }
        if let Some(p) = route.label_pos {
            let _ = write!(out, ", label-pos: ({:.2}pt, {:.2}pt)", p.x, p.y);
        }
        out.push_str(", path: (");
        for (i, (c1, c2, end)) in route.segments.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(
                out,
                "(c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt), end: ({:.2}pt, {:.2}pt))",
                c1.x, c1.y, c2.x, c2.y, end.x, end.y
            );
        }
        if route.segments.len() == 1 {
            out.push(',');
        }
        out.push_str(")),\n");
    }
    out.push_str("  ),\n)\n");
}
