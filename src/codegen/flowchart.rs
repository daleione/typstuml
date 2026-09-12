//! Flowchart-specific measurement, layout policy and Typst emission.
//! Semantic IR never passes through CUCA; only geometric algorithms are shared.
mod emit;
mod geom;
mod probe;

use super::graph::{
    compound::{self, Cluster},
    placement,
    routing::{self, LabelPlacement, RouteEdge, RoutingInput, RoutingMode, RoutingOptions},
};
use crate::ir::{FlowchartDiagram, LayoutDirection};
use crate::layout::{geometry::Point, graph::Orientation, spacing::Spacing};
use crate::runtime::MeasurementSet;
pub(super) use probe::{collect as collect_probes, has_probes};

pub(super) fn emit(
    out: &mut String,
    diag: &FlowchartDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    if diag.nodes.is_empty() {
        out.push_str("// (empty flowchart)\n");
        return;
    }
    let geoms: Vec<_> = diag
        .nodes
        .iter()
        .map(|node| {
            measurements
                .and_then(|s| s.get(&probe::node_id(diagram_idx, &node.id)))
                .map(|m| geom::measured(m.width_pt, m.height_pt))
                .unwrap_or_else(|| geom::estimate(node))
        })
        .collect();
    let index: std::collections::HashMap<_, _> = diag
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let edges: Vec<_> = diag
        .edges
        .iter()
        .enumerate()
        .filter_map(|(i, edge)| {
            Some((
                edge,
                RouteEdge {
                    src_idx: *index.get(edge.from.as_str())?,
                    dst_idx: *index.get(edge.to.as_str())?,
                    has_label: edge.label.is_some(),
                    label_size: edge
                        .label
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|label| {
                            measurements
                                .and_then(|s| s.get(&probe::edge_id(diagram_idx, i)))
                                .map(|m| Point::new(m.width_pt, m.height_pt))
                                .unwrap_or_else(|| geom::estimate_label(label))
                        }),
                },
            ))
        })
        .collect();
    let (semantic_edges, routes): (Vec<_>, Vec<_>) = edges.into_iter().unzip();
    let layout_edges: Vec<_> = routes
        .iter()
        .map(|e| {
            if diag.reverse_rank {
                (e.dst_idx, e.src_idx)
            } else {
                (e.src_idx, e.dst_idx)
            }
        })
        .collect();
    let sp = Spacing::for_font(10.0);
    let clusters: Vec<_> = diag
        .subgraphs
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let band = measurements.and_then(|s| s.get(&probe::subgraph_id(diagram_idx, i)));
            Cluster {
                nodes: group
                    .nodes
                    .iter()
                    .filter_map(|id| index.get(id.as_str()).copied())
                    .collect(),
                children: group.children.clone(),
                pad: sp.cluster_pad,
                label_band: band
                    .as_ref()
                    .map(|m| m.height_pt + 6.0)
                    .unwrap_or(sp.cluster_label_extra),
                label_min_width: band.map(|m| m.width_pt + 12.0).unwrap_or(0.0),
            }
        })
        .collect();
    let is_lr = diag.direction == LayoutDirection::LeftToRight;
    let orientation = if is_lr {
        Orientation::LeftToRight
    } else {
        Orientation::TopToBottom
    };
    let mut layout = compound::compound_layout(&clusters, &geoms, orientation, &layout_edges, sp);
    if !diag.reverse_rank {
        placement::recenter(
            &mut layout.top_lefts,
            &geoms,
            &layout_edges,
            &layout.entity_container,
            &layout.container_bboxes,
        );
    }
    let constraints: Vec<_> = diag
        .nodes
        .iter()
        .map(|n| geom::constraints(n.shape))
        .collect();
    let routed = routing::route(
        RoutingInput {
            geoms: &geoms,
            top_lefts: &layout.top_lefts,
            edges: &routes,
            constraints: &constraints,
            is_lr,
            foreign_obstacles: &|_, _| Vec::new(),
        },
        RoutingOptions {
            mode: RoutingMode::Spline,
            spacing: sp,
            exterior_self_loops: true,
            labels: LabelPlacement::AvoidOverlaps,
        },
    );
    emit::diagram(
        out,
        diag,
        &geoms,
        &layout,
        &semantic_edges,
        &routes,
        &routed,
    );
}
