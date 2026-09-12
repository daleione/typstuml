//! CUCA cluster membership and label bands adapted to the shared graph layout.
//! Tests here verify that CUCA packages retain their containment semantics.

use crate::codegen::graph::compound::{self, Cluster};
use crate::ir::CucaDiagram;
#[cfg(test)]
use crate::layout::geometry::Point;
use crate::layout::graph::Orientation;
use crate::layout::spacing::Spacing;
pub(super) use compound::LayoutResult;

use super::geom::{ClassGeom, FONT_PT};

/// This diagram family's spacing table (docs/cuca-architecture-layout-
/// redesign.md §3.1) — the ELK recipe scaled to our font size.
pub(super) fn spacing() -> Spacing {
    Spacing::for_font(FONT_PT)
}

/// Padding between a container's outer rectangle and its inner content.
pub(super) fn container_pad_pt() -> f64 {
    spacing().cluster_pad
}
/// Reserved band at the top of a container for the header label.
/// `together` (anonymous) gets 0; everything else gets this band as a
/// heuristic default. The measure protocol overrides this per-container
/// with `label_h + LABEL_BAND_PADDING_PT` so long / multi-line labels
/// get an appropriately tall band.
pub(super) fn container_label_pt() -> f64 {
    spacing().cluster_label_extra
}
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

pub(super) fn compound_layout(
    diag: &CucaDiagram,
    geoms: &[ClassGeom],
    orientation: Orientation,
    layout_edges: &[(usize, usize)],
    bands: LabelBands,
) -> LayoutResult {
    let clusters: Vec<_> = diag
        .containers
        .iter()
        .enumerate()
        .map(|(ci, c)| Cluster {
            nodes: diag
                .entities
                .iter()
                .enumerate()
                .filter(|(_, e)| c.children_entities.contains(&e.id))
                .map(|(i, _)| i)
                .collect(),
            children: c.children_containers.clone(),
            pad: container_pad_pt(),
            label_band: cluster_label_band_for_map(c, bands.get(ci)),
            label_min_width: bands
                .get(ci)
                .and_then(|b| b.as_ref())
                .map(|b| b.w_pt + 2.0 * LABEL_BAND_INSET_PT)
                .unwrap_or(0.0),
        })
        .collect();
    compound::compound_layout(&clusters, geoms, orientation, layout_edges, spacing())
}

/// Mirror of `cluster_label_band` but reads from an `Option<&LabelBand>`
/// directly — the hierarchical path doesn't have `cluster_data` to thread
/// through.
pub(super) fn cluster_label_band_for_map(
    c: &crate::ir::Container,
    band: Option<&Option<LabelBand>>,
) -> f64 {
    if c.together {
        return 0.0;
    }
    band.and_then(|b| b.as_ref())
        .map(|b| b.h_pt + LABEL_BAND_PADDING_PT)
        .unwrap_or_else(container_label_pt)
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

    fn pkg_with_children(label: &str, entities: Vec<String>, containers: Vec<usize>) -> Container {
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
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[(0, 1)], &[]);
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
        diag.containers.push(pkg("inner", vec!["Inner".into()]));
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
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[], &[]);

        let gp = result.container_bboxes[0].expect("grandparent bbox");
        let p = result.container_bboxes[1].expect("parent bbox");
        let c = result.container_bboxes[2].expect("child bbox");
        let inside = |inner: (Point, Point), outer: (Point, Point)| {
            inner.0.x >= outer.0.x - 1e-3
                && inner.0.y >= outer.0.y - 1e-3
                && inner.1.x <= outer.1.x + 1e-3
                && inner.1.y <= outer.1.y + 1e-3
        };
        assert!(
            inside(c, p),
            "child must sit inside parent; got c={c:?} p={p:?}"
        );
        assert!(
            inside(p, gp),
            "parent must sit inside grandparent; got p={p:?} gp={gp:?}"
        );
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
        diag.containers
            .push(pkg("PkgA", vec!["A".into(), "B".into()]));
        let geoms = vec![unit_geom(), unit_geom()];
        let result = compound_layout(&diag, &geoms, Orientation::TopToBottom, &[(0, 1)], &[]);
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
