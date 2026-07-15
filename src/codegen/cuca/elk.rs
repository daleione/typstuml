//! E9: desc-flavor (architecture) placement + orthogonal routing via the
//! ported ELK engine (`crate::layout::elk`), replacing the Sugiyama +
//! grid-router path for this diagram family. The adapter consumes a
//! *measured* model — entity sizes come from the same Typst-probe
//! geometry the rest of codegen uses — and returns absolute node boxes,
//! package frames and routed edge polylines in one pass, matching
//! draw-uml's engine behavior (verified against the elkjs oracle in
//! `tests/elk_port.rs`).

use std::collections::HashMap;

use crate::ir::{CucaDiagram, USymbol};
use crate::layout::elk::adapter::{
    self, AdapterEdge, AdapterGroup, AdapterModel, AdapterNode, AdapterSpacing,
};
use crate::layout::geometry::Point;
use crate::layout::ortho;

use super::emit::emit_edge;
use super::geom::{ClassGeom, Side, LOLLIPOP_DIAMETER_PT};
use super::layout::{cluster_label_band_for_map, container_pad_pt, spacing, LabelBands};
use super::theme::LineMode;
use super::OrientedEdge;

/// ELK-engine layout result, shaped like `layout::LayoutResult` plus the
/// routed polylines (one per layout edge, same order as the input).
pub(super) struct ElkDescLayout {
    pub top_lefts: Vec<Point>,
    pub container_bboxes: Vec<Option<(Point, Point)>>,
    pub entity_container: Vec<Option<usize>>,
    pub edge_points: Vec<Vec<Point>>,
}

fn node_id(i: usize) -> String {
    format!("n{i}")
}
fn group_id(i: usize) -> String {
    format!("g{i}")
}
fn edge_id(i: usize) -> String {
    format!("e{i}")
}

/// Whether container `ci` transitively holds any entity — empty
/// containers stay out of the ELK model (they'd import as zero-size
/// leaves) and keep their `None` bbox, like the Sugiyama path.
fn container_has_content(diag: &CucaDiagram, ci: usize) -> bool {
    !diag.containers[ci].children_entities.is_empty()
        || diag.containers[ci]
            .children_containers
            .iter()
            .any(|&cj| container_has_content(diag, cj))
}

/// Run the ELK engine on a desc-flavor diagram. `layout_edges` are the
/// oriented (source, target) entity-index pairs; self-loops and couple
/// edges are the caller's responsibility to exclude (gated in
/// `cuca::emit`).
pub(super) fn layout(
    diag: &CucaDiagram,
    geoms: &[ClassGeom],
    layout_edges: &[(usize, usize)],
    bands: LabelBands,
) -> ElkDescLayout {
    let sp = spacing();
    let mut model = AdapterModel {
        spacing: AdapterSpacing {
            node_gap: sp.node_node,
            layer_gap: sp.between_layers,
            content_pad: sp.root_node_node,
            edge_gap: sp.edge_label,
        },
        ..Default::default()
    };

    // Entities → measured adapter nodes. Lollipops (bare interfaces)
    // shrink to the disc with the label as an outside bottom label, so
    // edges route to the disc — draw-uml's icon-node mechanism.
    let entity_index: HashMap<&str, usize> =
        diag.entities.iter().enumerate().map(|(i, e)| (e.id.as_str(), i)).collect();
    for (i, entity) in diag.entities.iter().enumerate() {
        let size = geoms[i].size;
        if entity.usymbol == USymbol::Interface {
            model.nodes.push(AdapterNode {
                id: node_id(i),
                width: size.x,
                height: size.y,
                graphic: Some((LOLLIPOP_DIAMETER_PT, LOLLIPOP_DIAMETER_PT)),
                node_label: Some(entity.display.clone()),
            });
        } else {
            model.nodes.push(AdapterNode {
                id: node_id(i),
                width: size.x,
                height: size.y,
                graphic: None,
                node_label: None,
            });
        }
    }

    // Containers → groups (declaration order; entities before nested
    // sub-packages within each group, mirroring the IR's split lists).
    let pad = container_pad_pt();
    for (ci, c) in diag.containers.iter().enumerate() {
        if !container_has_content(diag, ci) {
            continue;
        }
        let band = cluster_label_band_for_map(c, bands.get(ci));
        let mut children: Vec<String> = c
            .children_entities
            .iter()
            .filter_map(|id| entity_index.get(id.as_str()).map(|&i| node_id(i)))
            .collect();
        children.extend(
            c.children_containers
                .iter()
                .filter(|&&cj| container_has_content(diag, cj))
                .map(|&cj| group_id(cj)),
        );
        model.groups.push(AdapterGroup {
            id: group_id(ci),
            children,
            // User-frame padding: the label band plus the uniform
            // cluster pad on top, the plain pad elsewhere — the same
            // insets the Sugiyama path bakes into its cluster frames.
            padding: (band + pad, pad, pad, pad),
        });
    }

    for (k, &(src, dst)) in layout_edges.iter().enumerate() {
        model.edges.push(AdapterEdge {
            id: edge_id(k),
            from: node_id(src),
            to: node_id(dst),
            labels: Vec::new(),
            inverted: false,
        });
    }

    let layout = adapter::layout_model(&model);

    let top_lefts: Vec<Point> = (0..diag.entities.len())
        .map(|i| {
            let n = &layout.nodes[&node_id(i)];
            Point::new(n.x, n.y)
        })
        .collect();
    let container_bboxes: Vec<Option<(Point, Point)>> = (0..diag.containers.len())
        .map(|ci| {
            layout.groups.get(&group_id(ci)).map(|g| {
                (Point::new(g.x, g.y), Point::new(g.x + g.width, g.y + g.height))
            })
        })
        .collect();
    // Innermost direct container per entity (same rule as the Sugiyama
    // path) — the emit-side containment helpers read this.
    let entity_container: Vec<Option<usize>> = diag
        .entities
        .iter()
        .map(|e| {
            diag.containers
                .iter()
                .enumerate()
                .rev()
                .find(|(_, c)| c.children_entities.iter().any(|id| id == &e.id))
                .map(|(i, _)| i)
        })
        .collect();

    let by_id: HashMap<&str, &adapter::LayoutEdge> =
        layout.edges.iter().map(|e| (e.id.as_str(), e)).collect();
    let edge_points: Vec<Vec<Point>> = (0..layout_edges.len())
        .map(|k| {
            let id = edge_id(k);
            by_id
                .get(id.as_str())
                .map(|e| e.points.iter().map(|&(x, y)| Point::new(x, y)).collect())
                .unwrap_or_default()
        })
        .collect();

    ElkDescLayout { top_lefts, container_bboxes, entity_container, edge_points }
}

/// Emit every oriented edge from its engine-routed polyline: sides come
/// from the first/last segment directions (the engine anchors endpoints
/// on node faces — the disc for lollipops), the free-axis override pins
/// the painter's anchor to the engine's exact coordinate, the polyline
/// rounds through the same `to_rounded_cubics` as the grid router, and
/// labeled edges get the longest-trunk midpoint. Trunk separation
/// already happened inside the engine (`separateOverlappingEdges`).
///
/// Actor / use-case endpoints keep the straight-spline look (§3.4 of the
/// redesign doc: stick figures and ellipses read poorly with orthogonal
/// jogs) — their edges collapse to a direct cubic between the engine's
/// anchor points, so placement still benefits from the engine.
pub(super) fn emit_edges(
    out: &mut String,
    diag: &CucaDiagram,
    oriented: &[OrientedEdge],
    elk: &ElkDescLayout,
    line_mode: LineMode,
) {
    let sp = spacing();
    let arc = if line_mode == LineMode::Polyline { 0.0 } else { sp.ortho_arc };

    // Direction of an axis-aligned segment, named by the face it leaves.
    let leave_dir = |a: Point, b: Point| -> Side {
        if (b.y - a.y).abs() >= (b.x - a.x).abs() {
            if b.y >= a.y { Side::Bot } else { Side::Top }
        } else if b.x >= a.x {
            Side::Right
        } else {
            Side::Left
        }
    };
    let prefers_spline = |i: usize| {
        matches!(
            diag.entities[i].usymbol,
            USymbol::Actor
                | USymbol::ActorBusiness
                | USymbol::ActorAwesome
                | USymbol::ActorHollow
                | USymbol::UseCase
                | USymbol::UseCaseBusiness
        )
    };

    for (k, oe) in oriented.iter().enumerate() {
        let pts = &elk.edge_points[k];
        if pts.len() < 2 {
            // No polyline from the engine (degenerate input) — emit a
            // straight segment between box centers as a safety net.
            let start = elk.top_lefts[oe.src_idx];
            let end = elk.top_lefts[oe.dst_idx];
            let mid = Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0);
            emit_edge(out, oe, &[(mid, mid, end)], None, None, None, None);
            continue;
        }

        let from_side = leave_dir(pts[0], pts[1]);
        // The arriving direction enters the opposite face.
        let to_side = match leave_dir(pts[pts.len() - 2], pts[pts.len() - 1]) {
            Side::Bot => Side::Top,
            Side::Top => Side::Bot,
            Side::Right => Side::Left,
            Side::Left => Side::Right,
        };
        let free = |side: Side, p: Point| match side {
            Side::Top | Side::Bot => p.x,
            Side::Left | Side::Right => p.y,
        };
        let from_override = Some(free(from_side, pts[0]));
        let to_override = Some(free(to_side, pts[pts.len() - 1]));

        if prefers_spline(oe.src_idx) || prefers_spline(oe.dst_idx) {
            let (start, end) = (pts[0], pts[pts.len() - 1]);
            let third = Point::new(
                start.x + (end.x - start.x) / 3.0,
                start.y + (end.y - start.y) / 3.0,
            );
            let two_thirds = Point::new(
                start.x + 2.0 * (end.x - start.x) / 3.0,
                start.y + 2.0 * (end.y - start.y) / 3.0,
            );
            emit_edge(
                out,
                oe,
                &[(third, two_thirds, end)],
                Some((from_side, to_side)),
                from_override,
                to_override,
                None,
            );
            continue;
        }

        let label_pos =
            oe.relation.label.as_ref().and_then(|_| ortho::longest_trunk_midpoint(pts));
        let segments = ortho::to_rounded_cubics(pts, arc);
        emit_edge(
            out,
            oe,
            &segments,
            Some((from_side, to_side)),
            from_override,
            to_override,
            label_pos,
        );
    }
}
