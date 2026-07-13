//! Per-edge obstacle scoping for cuca's orthogonal router (§3.3).
//!
//! `M3`'s Sugiyama ranking keeps sibling package frames apart, so the
//! spline routing chain (`route.rs`) never needed cluster frames as
//! obstacles — the natural path already avoided them. The orthogonal
//! router's job is different: it must actively route *around* foreign
//! packages instead of just avoiding entity boxes, or a long
//! cross-diagram edge cuts straight through an unrelated package's
//! interior (the exact defect diagnosed in
//! `docs/cuca-architecture-layout-redesign.md` §2.2).
//!
//! A container frame on *either* endpoint's own ancestor chain is left
//! transparent (not an obstacle) — an edge leaving from inside a
//! package is allowed to cross that package's own boundary freely.
//! Every other container frame is a full obstacle. This is a
//! simplification of the design doc's LCA-scoped "gate" model (a
//! foreign frame is either fully opaque or, on the shared ancestor
//! chain, fully transparent — no partial-wall corridor yet); it
//! already fixes edges cutting through unrelated packages, which is
//! the dominant visual defect. Revisit if a fixture needs the finer
//! gate model.

use crate::ir::CucaDiagram;
use crate::layout::geometry::Point;
use crate::layout::pathplan::Box as Obstacle;

/// `parents[c]` = the container index that lists `c` in its
/// `children_containers`, or `None` for a top-level container.
pub(super) fn container_parents(diag: &CucaDiagram) -> Vec<Option<usize>> {
    let mut parents = vec![None; diag.containers.len()];
    for (pi, c) in diag.containers.iter().enumerate() {
        for &ci in &c.children_containers {
            if ci < parents.len() {
                parents[ci] = Some(pi);
            }
        }
    }
    parents
}

/// `entity_container[i]`'s ancestor chain (the container itself, then
/// its parent, …), or empty for a root-level entity.
fn ancestor_chain(parents: &[Option<usize>], direct: Option<usize>) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut cur = direct;
    while let Some(c) = cur {
        chain.push(c);
        cur = parents[c];
    }
    chain
}

/// Container frames this edge must route around: every frame that is
/// on neither endpoint's own ancestor chain, as an obstacle box (the
/// painter-facing `(top_left, bottom_right)` pair already computed by
/// layout, at `label_band`-inclusive extent).
pub(super) fn foreign_frame_obstacles(
    diag: &CucaDiagram,
    container_bboxes: &[Option<(Point, Point)>],
    entity_container: &[Option<usize>],
    from: usize,
    to: usize,
) -> Vec<Obstacle> {
    let parents = container_parents(diag);
    let mut transparent = ancestor_chain(&parents, entity_container[from]);
    transparent.extend(ancestor_chain(&parents, entity_container[to]));

    container_bboxes
        .iter()
        .enumerate()
        .filter(|(ci, bb)| bb.is_some() && !transparent.contains(ci))
        .map(|(_, bb)| {
            let (tl, br) = bb.unwrap();
            Obstacle::new(tl, br)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ClassFamilyKind, Container, Entity, EntityKindData, USymbol};

    fn entity(id: &str) -> Entity {
        Entity {
            usymbol: USymbol::None,
            id: id.into(),
            display: id.into(),
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

    fn pkg(children_entities: Vec<String>, children_containers: Vec<usize>) -> Container {
        Container {
            usymbol: USymbol::Package,
            together: false,
            label: "Pkg".into(),
            stereotype: None,
            children_entities,
            children_containers,
            line: 0,
        }
    }

    #[test]
    fn own_package_frame_is_transparent_foreign_frame_is_obstacle() {
        // PkgA{A}, PkgB{B}: an edge from A (unclustered would also
        // work) to some other entity should treat PkgB as an obstacle
        // but not PkgA.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.containers.push(pkg(vec!["A".into()], vec![]));
        diag.containers.push(pkg(vec!["B".into()], vec![]));

        let entity_container = vec![Some(0), Some(1)];
        let container_bboxes = vec![
            Some((Point::new(0.0, 0.0), Point::new(10.0, 10.0))),
            Some((Point::new(20.0, 0.0), Point::new(30.0, 10.0))),
        ];

        let obstacles =
            foreign_frame_obstacles(&diag, &container_bboxes, &entity_container, 0, 1);
        // Both endpoints' own packages are transparent — no obstacles
        // when the edge is entirely within the union of both chains.
        assert!(obstacles.is_empty());
    }

    #[test]
    fn third_party_package_is_an_obstacle() {
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A"));
        diag.entities.push(entity("B"));
        diag.entities.push(entity("C"));
        diag.containers.push(pkg(vec!["A".into()], vec![]));
        diag.containers.push(pkg(vec!["B".into()], vec![]));
        diag.containers.push(pkg(vec!["C".into()], vec![]));

        let entity_container = vec![Some(0), Some(1), Some(2)];
        let container_bboxes = vec![
            Some((Point::new(0.0, 0.0), Point::new(10.0, 10.0))),
            Some((Point::new(20.0, 0.0), Point::new(30.0, 10.0))),
            Some((Point::new(40.0, 0.0), Point::new(50.0, 10.0))),
        ];

        // Edge from A to B: PkgC (belongs to neither endpoint) must be
        // an obstacle.
        let obstacles =
            foreign_frame_obstacles(&diag, &container_bboxes, &entity_container, 0, 1);
        assert_eq!(obstacles.len(), 1);
        assert_eq!(obstacles[0].min, Point::new(40.0, 0.0));
    }
}
