//! Compound layout for class diagrams.
//!
//! Two-level Sugiyama: each container runs its own sub-layout (over
//! its direct member entities and direct child containers treated as
//! opaque super-nodes), then a super-Sugiyama treats every top-level
//! container as a single box and places non-clustered entities
//! alongside. With no containers it degenerates to `flat_layout`.

use crate::ir::{ClassDiagram, ContainerKind};
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};

use super::geom::ClassGeom;

/// Padding between a container's outer rectangle and its inner content.
pub(super) const CONTAINER_PAD_PT: f64 = 14.0;
/// Reserved band at the top of a container for the header label.
/// `together` (anonymous) gets 0; everything else gets this band as a
/// heuristic default. The measure protocol overrides this per-container
/// with `label_h + LABEL_BAND_PADDING_PT` so long / multi-line labels
/// get an appropriately tall band.
pub(super) const CONTAINER_LABEL_PT: f64 = 14.0;
/// Vertical padding around a measured package label inside its band:
/// painter draws label at `dy: 2pt` from the band top, so 2pt above +
/// label_h + 2pt below + 2pt buffer before content = label_h + 6pt.
const LABEL_BAND_PADDING_PT: f64 = 6.0;
/// Horizontal padding around a measured package label so the label
/// (which the painter inset-places 6pt from each side) doesn't get
/// clipped by a too-narrow outer box.
const LABEL_BAND_INSET_PT: f64 = 6.0;

/// Per-container label measurements pulled from the `MeasurementSet`.
/// `None` for `together` (no band) or when the protocol is disabled —
/// the caller falls back to `CONTAINER_LABEL_PT` and no min-width.
pub(super) type LabelBands<'a> = &'a [Option<LabelBand>];

#[derive(Copy, Clone, Debug)]
pub(super) struct LabelBand {
    /// Measured natural width of the label text content (no insets).
    pub w_pt: f64,
    /// Measured natural height of the label text content (no insets).
    pub h_pt: f64,
}

/// Output of compound layout: per-entity absolute top-left position
/// plus per-container absolute outer bbox (None for empty containers).
pub(super) struct LayoutResult {
    pub top_lefts: Vec<Point>,
    pub container_bboxes: Vec<Option<(Point, Point)>>,
}

/// Per-cluster sub-layout result: positions of direct member entities
/// and direct child containers in the cluster's local frame (origin =
/// inner content top-left, with `cluster_content_offset` already
/// subtracted out), plus the inner content bbox.
struct ClusterData {
    members: Vec<(usize, Point)>,
    children: Vec<(usize, Point)>,
    inner_size: Point,
}

fn cluster_label_band(ci: usize, diag: &ClassDiagram, bands: LabelBands) -> f64 {
    if matches!(diag.containers[ci].kind, ContainerKind::Together) {
        return 0.0;
    }
    bands
        .get(ci)
        .and_then(|b| b.as_ref())
        .map(|b| b.h_pt + LABEL_BAND_PADDING_PT)
        .unwrap_or(CONTAINER_LABEL_PT)
}

/// Minimum outer width imposed by the label, so a wide / long package
/// title can't overflow a container with narrow contents. Zero when
/// no measurement is available (the painter just clips).
fn cluster_label_min_outer_w(ci: usize, bands: LabelBands) -> f64 {
    bands
        .get(ci)
        .and_then(|b| b.as_ref())
        .map(|b| b.w_pt + 2.0 * LABEL_BAND_INSET_PT)
        .unwrap_or(0.0)
}

fn cluster_outer_size(
    ci: usize,
    diag: &ClassDiagram,
    cluster_data: &std::collections::HashMap<usize, ClusterData>,
    bands: LabelBands,
) -> Point {
    let inner = cluster_data[&ci].inner_size;
    let pad = CONTAINER_PAD_PT;
    let band = cluster_label_band(ci, diag, bands);
    let min_label_w = cluster_label_min_outer_w(ci, bands);
    let outer_w = (inner.x + 2.0 * pad).max(min_label_w);
    Point::new(outer_w, inner.y + 2.0 * pad + band)
}

fn cluster_content_offset(ci: usize, diag: &ClassDiagram, bands: LabelBands) -> Point {
    let pad = CONTAINER_PAD_PT;
    let band = cluster_label_band(ci, diag, bands);
    Point::new(pad, pad + band)
}

/// Top-level container indices: those not registered as a child of any
/// other container.
fn top_level_containers(diag: &ClassDiagram) -> Vec<usize> {
    let mut is_child = vec![false; diag.containers.len()];
    for c in &diag.containers {
        for &cc in &c.children_containers {
            if cc < is_child.len() {
                is_child[cc] = true;
            }
        }
    }
    (0..diag.containers.len())
        .filter(|i| !is_child[*i])
        .collect()
}

/// For each entity, the chain of containers from the outermost root
/// down to the innermost cluster that contains it. Empty for entities
/// outside every container.
fn entity_cluster_chains(diag: &ClassDiagram) -> Vec<Vec<usize>> {
    let mut parent: Vec<Option<usize>> = vec![None; diag.containers.len()];
    for (pi, c) in diag.containers.iter().enumerate() {
        for &ci in &c.children_containers {
            if ci < parent.len() {
                parent[ci] = Some(pi);
            }
        }
    }
    diag.entities
        .iter()
        .map(|e| {
            let direct = diag
                .containers
                .iter()
                .enumerate()
                .rev()
                .find(|(_, c)| c.children_entities.iter().any(|cid| cid == &e.id))
                .map(|(i, _)| i);
            let mut chain = Vec::new();
            let mut cur = direct;
            while let Some(c) = cur {
                chain.push(c);
                cur = parent[c];
            }
            chain.reverse();
            chain
        })
        .collect()
}

/// Single flat Sugiyama, used when there are no containers. Same shape
/// as `compound_layout`'s output so callers don't branch.
fn flat_layout(
    diag: &ClassDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
) -> LayoutResult {
    let mut vg = VisualGraph::new(orientation);
    let handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node(Element::new_box(g.size, orientation)))
        .collect();
    for &(src, dst) in layout_edges {
        vg.add_edge(Edge::default(), handles[src], handles[dst]);
    }
    vg.layout();
    let top_lefts: Vec<Point> = handles.iter().map(|h| vg.pos(*h).bbox(false).0).collect();
    LayoutResult {
        top_lefts,
        container_bboxes: vec![None; diag.containers.len()],
    }
}

/// Compound graph layout (graphviz-style):
///
/// 1. **Per-cluster sub-Sugiyama**: each container is laid out
///    independently using only its direct member entities and its
///    direct child containers (treated as opaque super-nodes sized by
///    their already-computed outer bbox). Recursion handles nesting.
///
/// 2. **Super-Sugiyama**: top-level containers and any non-clustered
///    entities form a super-graph; cross-cluster relations become
///    super-edges between their endpoints' top-level supernodes.
///
/// 3. **Compose**: each entity's absolute position = its top-level
///    super-node origin + content offset + (recursive) sub-layout
///    offset. Container bboxes fall out as the supernode's outer
///    extent at each level.
///
/// This guarantees container rectangles never overlap, even when one
/// cluster's widest member is wider than another cluster's narrowest —
/// the post-Sugiyama "regroup by rank" hack the previous codegen used
/// could not enforce that.
pub(super) fn compound_layout(
    diag: &ClassDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    bands: LabelBands,
) -> LayoutResult {
    if diag.containers.is_empty() {
        return flat_layout(diag, geoms, orientation, layout_edges);
    }

    let chains = entity_cluster_chains(diag);
    let top_clusters = top_level_containers(diag);

    let mut cluster_data: std::collections::HashMap<usize, ClusterData> =
        std::collections::HashMap::new();
    for &ti in &top_clusters {
        layout_cluster(
            ti,
            diag,
            geoms,
            orientation,
            layout_edges,
            &chains,
            &mut cluster_data,
            bands,
        );
    }

    // Super-graph: each top-level cluster is one box; non-clustered
    // entities are individual boxes.
    let mut super_vg = VisualGraph::new(orientation);
    let mut super_h_for_cluster: std::collections::HashMap<usize, _> =
        std::collections::HashMap::new();
    let mut super_h_for_entity: std::collections::HashMap<usize, _> =
        std::collections::HashMap::new();

    for &ti in &top_clusters {
        if !cluster_data.contains_key(&ti) {
            continue;
        }
        let outer = cluster_outer_size(ti, diag, &cluster_data, bands);
        let h = super_vg.add_node(Element::new_box(outer, orientation));
        super_h_for_cluster.insert(ti, h);
    }
    for (ei, _) in diag.entities.iter().enumerate() {
        if chains[ei].is_empty() {
            let h = super_vg.add_node(Element::new_box(geoms[ei].size, orientation));
            super_h_for_entity.insert(ei, h);
        }
    }

    let super_handle = |ei: usize| {
        if let Some(top) = chains[ei].first() {
            super_h_for_cluster.get(top).copied()
        } else {
            super_h_for_entity.get(&ei).copied()
        }
    };

    for &(src, dst) in layout_edges {
        // Drop cluster-to-cluster super-edges. They would otherwise
        // rank one cluster strictly above the other (Sugiyama gives
        // the source rank N, target rank N+1), even though both could
        // fit side-by-side at the same rank. PlantUML's default lays
        // sibling clusters out at the same rank in declaration order
        // and routes the cross-cluster edge through their sides.
        // Edges where at least one endpoint is non-clustered still
        // contribute (so e.g. `OuterClass → ClusterMember` keeps the
        // outer class above the cluster).
        let src_in_cluster = !chains[src].is_empty();
        let dst_in_cluster = !chains[dst].is_empty();
        if src_in_cluster && dst_in_cluster {
            continue;
        }
        if let (Some(s), Some(d)) = (super_handle(src), super_handle(dst)) {
            if s != d {
                super_vg.add_edge(Edge::default(), s, d);
            }
        }
    }
    super_vg.layout();

    // Compose absolute positions.
    let mut top_lefts = vec![Point::new(0.0, 0.0); diag.entities.len()];
    let mut container_bboxes: Vec<Option<(Point, Point)>> = vec![None; diag.containers.len()];

    for (&ti, &h) in &super_h_for_cluster {
        let outer_top_left = super_vg.pos(h).bbox(false).0;
        place_cluster(
            ti,
            outer_top_left,
            diag,
            &cluster_data,
            &mut top_lefts,
            &mut container_bboxes,
            bands,
        );
    }
    for (&ei, &h) in &super_h_for_entity {
        top_lefts[ei] = super_vg.pos(h).bbox(false).0;
    }

    LayoutResult {
        top_lefts,
        container_bboxes,
    }
}

/// Recursively lay out a cluster: child clusters first (so their bbox
/// is known), then a Sugiyama pass on this cluster's direct members +
/// child cluster super-nodes. Edges are restricted to those visible
/// from this cluster (both endpoints are direct or descend through a
/// direct child).
fn layout_cluster(
    ci: usize,
    diag: &ClassDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    chains: &[Vec<usize>],
    cluster_data: &mut std::collections::HashMap<usize, ClusterData>,
    bands: LabelBands,
) {
    if cluster_data.contains_key(&ci) {
        return;
    }
    // Recurse into nested children first.
    let child_indices = diag.containers[ci].children_containers.clone();
    for child in &child_indices {
        layout_cluster(
            *child,
            diag,
            geoms,
            orientation,
            layout_edges,
            chains,
            cluster_data,
            bands,
        );
    }

    let mut sub_vg = VisualGraph::new(orientation);
    let mut entity_h: std::collections::HashMap<usize, _> = std::collections::HashMap::new();
    let mut child_h: std::collections::HashMap<usize, _> = std::collections::HashMap::new();

    for child_id in &diag.containers[ci].children_entities {
        if let Some(ei) = diag.entities.iter().position(|e| &e.id == child_id) {
            let h = sub_vg.add_node(Element::new_box(geoms[ei].size, orientation));
            entity_h.insert(ei, h);
        }
    }
    for &cidx in &child_indices {
        if !cluster_data.contains_key(&cidx) {
            continue;
        }
        let outer = cluster_outer_size(cidx, diag, cluster_data, bands);
        let h = sub_vg.add_node(Element::new_box(outer, orientation));
        child_h.insert(cidx, h);
    }

    // Each entity's chain tells us which sub-graph node — if any — it
    // maps to from this cluster's perspective: a direct member if `ci`
    // is the chain's last element, else the child cluster that's one
    // level down from `ci` in the chain. Entities outside `ci` return
    // None (their edges become super-edges at a higher level).
    let endpoint = |ei: usize| {
        let chain = &chains[ei];
        let pos = chain.iter().position(|&c| c == ci)?;
        if pos == chain.len() - 1 {
            entity_h.get(&ei).copied()
        } else {
            let child_ci = chain[pos + 1];
            child_h.get(&child_ci).copied()
        }
    };

    for &(src, dst) in layout_edges {
        if let (Some(s), Some(d)) = (endpoint(src), endpoint(dst)) {
            if s != d {
                sub_vg.add_edge(Edge::default(), s, d);
            }
        }
    }
    sub_vg.layout();

    // Extract sub-positions in cluster-local frame, then normalize so
    // the bbox top-left lands at (0, 0).
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    let mut members: Vec<(usize, Point)> = entity_h
        .iter()
        .map(|(&ei, &h)| (ei, sub_vg.pos(h).bbox(false).0))
        .collect();
    members.sort_by_key(|&(ei, _)| ei);
    for &(ei, p) in &members {
        let s = geoms[ei].size;
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x + s.x);
        max_y = max_y.max(p.y + s.y);
    }

    let mut children: Vec<(usize, Point)> = child_h
        .iter()
        .map(|(&cidx, &h)| (cidx, sub_vg.pos(h).bbox(false).0))
        .collect();
    children.sort_by_key(|&(c, _)| c);
    for &(cidx, p) in &children {
        let outer = cluster_outer_size(cidx, diag, cluster_data, bands);
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x + outer.x);
        max_y = max_y.max(p.y + outer.y);
    }

    if members.is_empty() && children.is_empty() {
        cluster_data.insert(
            ci,
            ClusterData {
                members: Vec::new(),
                children: Vec::new(),
                inner_size: Point::new(0.0, 0.0),
            },
        );
        return;
    }

    for (_, p) in &mut members {
        p.x -= min_x;
        p.y -= min_y;
    }
    for (_, p) in &mut children {
        p.x -= min_x;
        p.y -= min_y;
    }

    cluster_data.insert(
        ci,
        ClusterData {
            members,
            children,
            inner_size: Point::new(max_x - min_x, max_y - min_y),
        },
    );
}

/// Walk a cluster (and its nested children) translating local
/// positions into absolute coordinates. `outer_top_left` is the
/// cluster's own outer rectangle origin in the absolute frame.
fn place_cluster(
    ci: usize,
    outer_top_left: Point,
    diag: &ClassDiagram,
    cluster_data: &std::collections::HashMap<usize, ClusterData>,
    top_lefts: &mut [Point],
    bboxes: &mut [Option<(Point, Point)>],
    bands: LabelBands,
) {
    let outer_size = cluster_outer_size(ci, diag, cluster_data, bands);
    bboxes[ci] = Some((outer_top_left, outer_top_left.add(outer_size)));
    let content_origin = outer_top_left.add(cluster_content_offset(ci, diag, bands));
    let data = &cluster_data[&ci];
    for &(ei, local) in &data.members {
        top_lefts[ei] = content_origin.add(local);
    }
    for &(child_ci, local) in &data.children {
        let child_outer = content_origin.add(local);
        place_cluster(
            child_ci,
            child_outer,
            diag,
            cluster_data,
            top_lefts,
            bboxes,
            bands,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ClassDiagram, Container, ContainerKind, Entity, EntityKind};

    fn entity(name: &str) -> Entity {
        Entity {
            kind: EntityKind::Class,
            id: name.into(),
            display: name.into(),
            generic: None,
            stereotype: None,
            stereotype_marker: None,
            fields: Vec::new(),
            methods: Vec::new(),
            body: None,
            fill: None,
            line: 0,
        }
    }

    fn unit_geom() -> ClassGeom {
        ClassGeom {
            size: Point::new(60.0, 40.0),
            mid_x: 30.0,
        }
    }

    #[test]
    fn flat_layout_when_no_containers() {
        // No containers → falls back to flat_layout, container_bboxes
        // is all-None, every entity gets a top-left.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        let geoms = vec![unit_geom(), unit_geom()];
        let result =
            compound_layout(&diag, &geoms, Orientation::TopToBottom, &[(0, 1)], &[]);
        assert_eq!(result.top_lefts.len(), 2);
        assert!(result.container_bboxes.is_empty());
    }

    #[test]
    fn compound_layout_places_entities_inside_their_clusters() {
        // Two sibling containers, one entity each, no relations.
        // Both clusters should get a bbox; each entity must sit inside
        // its declared cluster's bbox.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(Container {
            kind: ContainerKind::Package,
            label: "PkgA".into(),
            stereotype: None,
            children_entities: vec!["A".into()],
            children_containers: Vec::new(),
            line: 0,
        });
        diag.containers.push(Container {
            kind: ContainerKind::Package,
            label: "PkgB".into(),
            stereotype: None,
            children_entities: vec!["B".into()],
            children_containers: Vec::new(),
            line: 0,
        });
        let geoms = vec![unit_geom(), unit_geom()];
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[], &[]);

        let bb_a = result.container_bboxes[0].expect("PkgA bbox");
        let bb_b = result.container_bboxes[1].expect("PkgB bbox");
        let a_tl = result.top_lefts[0];
        let b_tl = result.top_lefts[1];
        let a_br = a_tl.add(geoms[0].size);
        let b_br = b_tl.add(geoms[1].size);

        let inside = |p_tl: Point, p_br: Point, bb: (Point, Point)| {
            p_tl.x >= bb.0.x - 1e-3
                && p_tl.y >= bb.0.y - 1e-3
                && p_br.x <= bb.1.x + 1e-3
                && p_br.y <= bb.1.y + 1e-3
        };
        assert!(inside(a_tl, a_br, bb_a), "A must sit inside PkgA bbox");
        assert!(inside(b_tl, b_br, bb_b), "B must sit inside PkgB bbox");
    }

    #[test]
    fn compound_layout_sibling_cluster_bboxes_disjoint() {
        // Same setup as above; verify cluster bboxes don't overlap.
        // This is the property the "drop cluster-to-cluster super-edges"
        // rule + super-Sugiyama is meant to enforce.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(Container {
            kind: ContainerKind::Package,
            label: "PkgA".into(),
            stereotype: None,
            children_entities: vec!["A".into()],
            children_containers: Vec::new(),
            line: 0,
        });
        diag.containers.push(Container {
            kind: ContainerKind::Package,
            label: "PkgB".into(),
            stereotype: None,
            children_entities: vec!["B".into()],
            children_containers: Vec::new(),
            line: 0,
        });
        let geoms = vec![unit_geom(), unit_geom()];
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[], &[]);
        let bb_a = result.container_bboxes[0].unwrap();
        let bb_b = result.container_bboxes[1].unwrap();
        // Disjoint along at least one axis.
        let disjoint = bb_a.1.x <= bb_b.0.x
            || bb_b.1.x <= bb_a.0.x
            || bb_a.1.y <= bb_b.0.y
            || bb_b.1.y <= bb_a.0.y;
        assert!(
            disjoint,
            "sibling cluster bboxes must not overlap; got {bb_a:?} vs {bb_b:?}"
        );
    }

    #[test]
    fn compound_layout_nested_cluster_contained_in_parent() {
        // outer { inner { Inner } } — the inner cluster's bbox must
        // sit inside the outer cluster's bbox.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("Inner"));
        diag.containers.push(Container {
            kind: ContainerKind::Namespace,
            label: "outer".into(),
            stereotype: None,
            children_entities: Vec::new(),
            children_containers: vec![1],
            line: 0,
        });
        diag.containers.push(Container {
            kind: ContainerKind::Namespace,
            label: "inner".into(),
            stereotype: None,
            children_entities: vec!["Inner".into()],
            children_containers: Vec::new(),
            line: 0,
        });
        let geoms = vec![unit_geom()];
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[], &[]);
        let outer = result.container_bboxes[0].unwrap();
        let inner = result.container_bboxes[1].unwrap();
        assert!(
            inner.0.x >= outer.0.x - 1e-3
                && inner.0.y >= outer.0.y - 1e-3
                && inner.1.x <= outer.1.x + 1e-3
                && inner.1.y <= outer.1.y + 1e-3,
            "inner cluster must be inside outer; got inner={inner:?} outer={outer:?}"
        );
    }
}
