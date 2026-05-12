//! Compound layout for cuca (description-family) diagrams.
//!
//! Single-pass hierarchical Sugiyama (M3): all entities live in one
//! `VisualGraph` with a side-band `HierarchyMap` recording cluster
//! membership. Cluster-to-cluster edges participate in ranking, so
//! `PkgA.Foo → PkgB.Bar` correctly places PkgA above PkgB in TB layout.
//! `simple` / `bk` / `compact` / `port_align` are cluster-oblivious;
//! the `tighten` pass closes the loop by computing per-cluster outer
//! bboxes after BK and shifting siblings apart when their pads overlap.
//!
//! With no containers, falls back to `flat_layout` (an empty
//! `HierarchyMap` makes the hierarchy-aware passes no-ops anyway).

use crate::ir::CucaDiagram;
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};
use crate::layout::sugiyama::hierarchy::HierarchyMap;

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

/// For each entity, the chain of containers from the outermost root
/// down to the innermost cluster that contains it. Empty for entities
/// outside every container.
pub(super) fn entity_cluster_chains(diag: &CucaDiagram) -> Vec<Vec<usize>> {
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
    diag: &CucaDiagram,
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

/// Compound graph layout (M3, single-pass hierarchical Sugiyama).
/// Dispatches to `flat_layout` when there are no containers and to
/// `hierarchical_layout` otherwise. Both return the same
/// `LayoutResult` shape so callers don't branch.
pub(super) fn compound_layout(
    diag: &CucaDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    bands: LabelBands,
) -> LayoutResult {
    if diag.containers.is_empty() {
        return flat_layout(diag, geoms, orientation, layout_edges);
    }
    hierarchical_layout(diag, geoms, orientation, layout_edges, bands)
}

/// Single-pass hierarchical Sugiyama (M3). All entities live in one
/// `VisualGraph`; cluster membership is recorded in `HierarchyMap` and
/// consulted by the row-grouping pass + the mincross same-cluster gate
/// + tighten. Cluster-to-cluster edges now participate in ranking
/// (replacing the old two-stage "drop cluster-to-cluster super-edges"
/// shortcut), so source / target clusters get ranked relative to each
/// other through their members' rank assignment.
fn hierarchical_layout(
    diag: &CucaDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    bands: LabelBands,
) -> LayoutResult {
    let mut vg = VisualGraph::new(orientation);
    let entity_handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node(Element::new_box(g.size, orientation)))
        .collect();
    for &(src, dst) in layout_edges {
        vg.add_edge(Edge::default(), entity_handles[src], entity_handles[dst]);
    }

    // Build the cluster map. We need a stable mapping from container index
    // (CucaDiagram order) to ClusterId; using the same indices keeps
    // downstream code (container_bboxes vec) trivially indexed.
    let mut hierarchy = HierarchyMap::new();
    // Two-pass: first add all clusters so parent pointers can reference
    // them; then wire children + node membership.
    for c in &diag.containers {
        // Parent is filled in pass 2.
        let _ = hierarchy.add_cluster(None);
        // Per-cluster geometric knobs feed `tighten`.
        let last = hierarchy.clusters.len() - 1;
        hierarchy.clusters[last].pad = CONTAINER_PAD_PT;
        hierarchy.clusters[last].label_band =
            if c.together { 0.0 } else { cluster_label_band_for_map(c, bands.get(last)) };
        hierarchy.clusters[last].label_min_w = bands
            .get(last)
            .and_then(|b| b.as_ref())
            .map(|b| b.w_pt + 2.0 * LABEL_BAND_INSET_PT)
            .unwrap_or(0.0);
    }
    for (pi, c) in diag.containers.iter().enumerate() {
        for &ci in &c.children_containers {
            if ci < hierarchy.clusters.len() {
                hierarchy.clusters[ci].parent = Some(pi);
                hierarchy.clusters[pi].direct_children.push(ci);
            }
        }
    }
    // Attach each entity to its innermost direct cluster (the container
    // whose `children_entities` lists it).
    for (ei, e) in diag.entities.iter().enumerate() {
        let direct = diag
            .containers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| c.children_entities.iter().any(|id| id == &e.id))
            .map(|(i, _)| i);
        if let Some(c) = direct {
            hierarchy.assign_node(entity_handles[ei], c);
        }
    }
    vg.set_hierarchy(hierarchy);

    vg.layout();

    // Extract per-entity top-lefts.
    let top_lefts: Vec<Point> = entity_handles
        .iter()
        .map(|h| vg.pos(*h).bbox(false).0)
        .collect();
    // Extract per-cluster outer bboxes; infinity sentinel → None.
    let container_bboxes: Vec<Option<(Point, Point)>> = (0..diag.containers.len())
        .map(|i| {
            let c = &vg.hierarchy.clusters[i];
            if c.x_min.is_finite() && c.x_max.is_finite() {
                Some((
                    Point::new(c.x_min, c.y_min),
                    Point::new(c.x_max, c.y_max),
                ))
            } else {
                None
            }
        })
        .collect();

    LayoutResult {
        top_lefts,
        container_bboxes,
    }
}

/// Mirror of `cluster_label_band` but reads from an `Option<&LabelBand>`
/// directly — the hierarchical path doesn't have `cluster_data` to thread
/// through.
fn cluster_label_band_for_map(c: &crate::ir::Container, band: Option<&Option<LabelBand>>) -> f64 {
    if c.together {
        return 0.0;
    }
    band.and_then(|b| b.as_ref())
        .map(|b| b.h_pt + LABEL_BAND_PADDING_PT)
        .unwrap_or(CONTAINER_LABEL_PT)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ClassFamilyKind, Container, CucaDiagram, Entity, EntityKindData, USymbol};

    fn entity(name: &str) -> Entity {
        Entity {
            usymbol: USymbol::None,
            id: name.into(),
            display: name.into(),
            stereotype: None,
            stereotype_marker: None,
            fill: None,
            line: 0,
            kind_data: EntityKindData::Compartment {
                kind: ClassFamilyKind::Class,
                generic: None,
                fields: Vec::new(),
                methods: Vec::new(),
            },
        }
    }

    fn pkg(label: &str, children: Vec<String>) -> Container {
        Container {
            usymbol: USymbol::Package,
            together: false,
            label: label.into(),
            stereotype: None,
            children_entities: children,
            children_containers: Vec::new(),
            line: 0,
        }
    }

    fn pkg_with_children(
        label: &str,
        entities: Vec<String>,
        containers: Vec<usize>,
    ) -> Container {
        Container {
            usymbol: USymbol::Package,
            together: false,
            label: label.into(),
            stereotype: None,
            children_entities: entities,
            children_containers: containers,
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
        let mut diag = CucaDiagram::default();
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
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(pkg("PkgA", vec!["A".into()]));
        diag.containers.push(pkg("PkgB", vec!["B".into()]));
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
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(pkg("PkgA", vec!["A".into()]));
        diag.containers.push(pkg("PkgB", vec!["B".into()]));
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
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("Inner"));
        diag.containers
            .push(pkg_with_children("outer", Vec::new(), vec![1]));
        diag.containers
            .push(pkg("inner", vec!["Inner".into()]));
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

    // ----- M3 tests -----

    #[test]
    fn compound_layout_3_level_nested_bboxes_strictly_contained() {
        // grandparent { parent { child { Leaf } } } — every level's
        // bbox must sit strictly inside the next outer level.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("Leaf"));
        // Containers ordered grandparent (0), parent (1), child (2).
        diag.containers
            .push(pkg_with_children("grandparent", Vec::new(), vec![1]));
        diag.containers
            .push(pkg_with_children("parent", Vec::new(), vec![2]));
        diag.containers.push(pkg("child", vec!["Leaf".into()]));
        let geoms = vec![unit_geom()];
        let result =
            compound_layout(&diag, &geoms, Orientation::TopToBottom, &[], &[]);

        let gp = result.container_bboxes[0].expect("grandparent bbox");
        let p = result.container_bboxes[1].expect("parent bbox");
        let c = result.container_bboxes[2].expect("child bbox");
        let inside = |inner: (Point, Point), outer: (Point, Point)| {
            inner.0.x >= outer.0.x - 1e-3
                && inner.0.y >= outer.0.y - 1e-3
                && inner.1.x <= outer.1.x + 1e-3
                && inner.1.y <= outer.1.y + 1e-3
        };
        assert!(inside(c, p), "child must sit inside parent; got c={c:?} p={p:?}");
        assert!(inside(p, gp), "parent must sit inside grandparent; got p={p:?} gp={gp:?}");
    }

    #[test]
    fn compound_layout_cross_cluster_edge_ranks_clusters() {
        // PkgA{A} → PkgB{B}: A → B forces PkgA above PkgB in TB
        // layout. Previously (M2 stopgap) cross-cluster edges were
        // dropped and the two clusters ended up side-by-side at the
        // same rank in declaration order.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(pkg("PkgA", vec!["A".into()]));
        diag.containers.push(pkg("PkgB", vec!["B".into()]));
        let geoms = vec![unit_geom(), unit_geom()];
        let result = compound_layout(
            &diag,
            &geoms,
            Orientation::TopToBottom,
            &[(0, 1)], // A → B
            &[],
        );
        let bb_a = result.container_bboxes[0].expect("PkgA bbox");
        let bb_b = result.container_bboxes[1].expect("PkgB bbox");
        // In TB, "A → B" means A above B, so PkgA's bbox must lie
        // entirely above PkgB's. Allow tolerance for shared edges
        // (they're disjoint already if they don't share a y, but the
        // strict "above" check below catches accidental reversals).
        assert!(
            bb_a.1.y <= bb_b.0.y + 1e-3,
            "PkgA must rank above PkgB; got PkgA={bb_a:?} PkgB={bb_b:?}"
        );
    }

    #[test]
    fn compound_layout_intra_cluster_edge_keeps_members_inside() {
        // PkgA{A, B}, edge A → B: both endpoints are inside the same
        // cluster, and the cluster's bbox must contain both. Verifies
        // that cluster_bubble / mincross gate / tighten cooperate to
        // keep an edge from pulling a member outside its cluster.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(pkg("PkgA", vec!["A".into(), "B".into()]));
        let geoms = vec![unit_geom(), unit_geom()];
        let result = compound_layout(
            &diag,
            &geoms,
            Orientation::TopToBottom,
            &[(0, 1)],
            &[],
        );
        let bb = result.container_bboxes[0].expect("PkgA bbox");
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
        assert!(inside(a_tl, a_br, bb), "A must sit inside PkgA");
        assert!(inside(b_tl, b_br, bb), "B must sit inside PkgA");
    }
}
