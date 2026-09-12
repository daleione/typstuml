//! Hierarchical Sugiyama placement from geometry and cluster membership.
use super::geom::NodeGeom;
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Orientation, VisualGraph};
use crate::layout::spacing::Spacing;
use crate::layout::sugiyama::hierarchy::HierarchyMap;

pub(crate) struct Cluster {
    pub nodes: Vec<usize>,
    pub children: Vec<usize>,
    pub pad: f64,
    pub label_band: f64,
    pub label_min_width: f64,
}

/// Output of compound layout: per-entity absolute top-left position,
/// per-container absolute outer bbox (None for empty containers), and
/// each entity's innermost direct container (None for root-level).
/// The last field lets codegen's post-layout heuristics (leaf
/// recenter, couple-edge chord alignment) keep an entity inside its
/// own frame and out of a foreign one without recomputing membership.
pub(crate) struct LayoutResult {
    pub top_lefts: Vec<Point>,
    pub container_bboxes: Vec<Option<(Point, Point)>>,
    pub entity_container: Vec<Option<usize>>,
}

/// Per-node halo for cuca's Sugiyama placement: `node_node` on the
/// perpendicular axis, `between_layers` on the rank axis, both from
/// this diagram's `Spacing`. `is_root` requests the reduced
/// `root_node_node` gap on the perpendicular axis — ELK's "root
/// nodeNode", used only when the diagram actually has containers:
/// packages already carry their own padding, so an unclustered node
/// sitting next to one doesn't need the full inter-node gap.
fn node_halo(sp: &Spacing, orientation: Orientation, is_root: bool) -> Point {
    let perp = if is_root {
        sp.root_node_node
    } else {
        sp.node_node
    };
    match orientation {
        Orientation::TopToBottom => Point::new(perp, sp.between_layers),
        Orientation::LeftToRight => Point::new(sp.between_layers, perp),
    }
}

/// Single flat Sugiyama, used when there are no containers. Same shape
/// as `compound_layout`'s output so callers don't branch.
fn flat_layout(
    clusters: &[Cluster],
    geoms: &[NodeGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    sp: Spacing,
) -> LayoutResult {
    let mut vg = VisualGraph::new(orientation);
    vg.set_spacing(sp);
    vg.set_model_order(true);
    let halo = node_halo(&sp, orientation, false);
    let handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node_with_halo(g.size, halo, orientation))
        .collect();
    for &(src, dst) in layout_edges {
        vg.add_edge(Edge::default(), handles[src], handles[dst]);
    }
    vg.layout();
    let top_lefts: Vec<Point> = handles.iter().map(|h| vg.pos(*h).bbox(false).0).collect();
    LayoutResult {
        top_lefts,
        container_bboxes: vec![None; clusters.len()],
        entity_container: vec![None; geoms.len()],
    }
}

/// Compound graph layout (M3, single-pass hierarchical Sugiyama).
/// Dispatches to `flat_layout` when there are no containers and to
/// `hierarchical_layout` otherwise. Both return the same
/// `LayoutResult` shape so callers don't branch.
pub(crate) fn compound_layout(
    clusters: &[Cluster],
    geoms: &[NodeGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    sp: Spacing,
) -> LayoutResult {
    if clusters.is_empty() {
        return flat_layout(clusters, geoms, orientation, layout_edges, sp);
    }
    hierarchical_layout(clusters, geoms, orientation, layout_edges, sp)
}

/// Single-pass hierarchical Sugiyama (M3). All entities live in one
/// `VisualGraph`; cluster membership is recorded in `HierarchyMap` and
/// consulted by row grouping, the mincross same-cluster gate, and
/// tightening. Cluster-to-cluster edges participate in ranking
/// (replacing the old two-stage "drop cluster-to-cluster super-edges"
/// shortcut), so source / target clusters get ranked relative to each
/// other through their members' rank assignment.
fn hierarchical_layout(
    clusters: &[Cluster],
    geoms: &[NodeGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    sp: Spacing,
) -> LayoutResult {
    let mut vg = VisualGraph::new(orientation);
    vg.set_spacing(sp);
    vg.set_model_order(true);

    // Innermost direct container for each entity (or `None` for a
    // root-level / unclustered entity), computed up front so node
    // construction can give root-level entities the reduced
    // `root_node_node` halo (§3.1) — packages already carry their own
    // padding, so an unclustered node next to one doesn't need the
    // full inter-node gap.
    let direct_container: Vec<Option<usize>> = (0..geoms.len())
        .map(|ei| clusters.iter().rposition(|c| c.nodes.contains(&ei)))
        .collect();

    let entity_handles: Vec<_> = geoms
        .iter()
        .enumerate()
        .map(|(ei, g)| {
            let halo = node_halo(&sp, orientation, direct_container[ei].is_none());
            vg.add_node_with_halo(g.size, halo, orientation)
        })
        .collect();
    for &(src, dst) in layout_edges {
        vg.add_edge(Edge::default(), entity_handles[src], entity_handles[dst]);
    }

    // Build the cluster map. We need a stable mapping from container index
    // (caller order) to ClusterId; using the same indices keeps
    // downstream code (container_bboxes vec) trivially indexed.
    let mut hierarchy = HierarchyMap::new();
    // Two-pass: first add all clusters so parent pointers can reference
    // them; then wire children + node membership.
    for c in clusters {
        // Parent is filled in pass 2.
        let _ = hierarchy.add_cluster(None);
        // Per-cluster geometric knobs feed `tighten`.
        let last = hierarchy.clusters.len() - 1;
        hierarchy.clusters[last].pad = c.pad;
        hierarchy.clusters[last].label_band = c.label_band;
        hierarchy.clusters[last].label_min_w = c.label_min_width;
    }
    for (pi, c) in clusters.iter().enumerate() {
        for &ci in &c.children {
            if ci < hierarchy.clusters.len() {
                hierarchy.clusters[ci].parent = Some(pi);
                hierarchy.clusters[pi].direct_children.push(ci);
            }
        }
    }
    // Attach each entity to its innermost direct cluster (the container
    // whose `children_entities` lists it) — reusing the membership
    // computed above for halo selection.
    for (ei, direct) in direct_container.iter().enumerate() {
        if let Some(c) = *direct {
            hierarchy.assign_node(entity_handles[ei], c);
        }
    }
    vg.set_hierarchy(hierarchy);
    vg.set_cluster_rank(true);

    vg.layout();

    // Extract per-entity top-lefts.
    let top_lefts: Vec<Point> = entity_handles
        .iter()
        .map(|h| vg.pos(*h).bbox(false).0)
        .collect();
    // Extract per-cluster outer bboxes; infinity sentinel → None.
    let container_bboxes: Vec<Option<(Point, Point)>> = (0..clusters.len())
        .map(|i| {
            let c = &vg.hierarchy.clusters[i];
            if c.x_min.is_finite() && c.x_max.is_finite() {
                Some((Point::new(c.x_min, c.y_min), Point::new(c.x_max, c.y_max)))
            } else {
                None
            }
        })
        .collect();

    LayoutResult {
        top_lefts,
        container_bboxes,
        entity_container: direct_container,
    }
}
