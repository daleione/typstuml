//! Mind-map diagram codegen.
//!
//! Splits the root's first-level children by [`NodeSide`], computes the
//! two-column mind-map geometry in Rust ([`crate::layout::tree`], a
//! faithful port of `tree.typ`'s `mindmap()` composition), and emits a
//! `#tree-layout(...)` call carrying absolute coordinates. Node sizes
//! come from the pass-1 measure protocol via [`super::tree_graph`].
//!
//! v1 simplification (per `docs/mindmap-wbs-plan.md` §3.2): a child whose
//! parsed side is `Default` (no explicit `+`/`-`, e.g. plain `**` markers)
//! is treated as right-side. PlantUML's auto-balance heuristic is left for
//! a follow-up.

use crate::codegen::tree_emit::emit_title;
use crate::codegen::tree_graph;
use crate::ir::{MindMapDiagram, NodeSide};
use crate::layout::tree::{layout_mindmap, TreeConfig, TreeLayoutInput};
use crate::runtime::MeasurementSet;

pub fn emit(
    out: &mut String,
    mm: &MindMapDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    if let Some(title) = &mm.title {
        emit_title(out, title);
    }

    let em = tree_graph::resolve_em(measurements, diagram_idx);
    let cfg = TreeConfig::from_em(em);

    // IDs are assigned in pre-order over the ORIGINAL child order (the
    // same order `flatten` / `collect_probes` walk); the side partition
    // below only affects which column a subtree lands in.
    let mut counter = 0;
    let root_id = counter;
    counter += 1;
    let root_size =
        tree_graph::resolve_node_size(&mm.root, root_id, measurements, diagram_idx, em);
    let root_input = TreeLayoutInput {
        id: root_id,
        size: root_size,
        children: Vec::new(),
    };

    let mut lefts: Vec<TreeLayoutInput> = Vec::new();
    let mut rights: Vec<TreeLayoutInput> = Vec::new();
    for child in &mm.root.children {
        let input = tree_graph::build_input(child, &mut counter, measurements, diagram_idx, em);
        match child.side {
            NodeSide::Left => lefts.push(input),
            // Right and Default both map to the right column in v1.
            NodeSide::Right | NodeSide::Default => rights.push(input),
        }
    }

    let layout = layout_mindmap(&root_input, &lefts, &rights, &cfg);
    let flat = tree_graph::flatten(&mm.root);

    out.push_str("#align(center, ");
    tree_graph::emit_layout_call(out, &layout, &flat);
    out.push_str(")\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{NodeShape, NodeSide, TreeNode};

    fn n(label: &str, side: NodeSide, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            label: vec![label.to_string()],
            side,
            shape: NodeShape::Box,
            fill: None,
            id: None,
            line: 0,
            children,
        }
    }

    fn render(mm: &MindMapDiagram) -> String {
        let mut s = String::new();
        emit(&mut s, mm, None, 0);
        s
    }

    #[test]
    fn root_only_emits_single_node_layout() {
        let s = render(&MindMapDiagram {
            name: None,
            title: None,
            root: n("Root", NodeSide::Default, vec![]),
        });
        assert!(s.contains("tree-layout("), "got: {s}");
        assert_eq!(s.matches("body: node[").count(), 1, "got: {s}");
        assert_eq!(s.matches("(points: (").count(), 0, "got: {s}");
    }

    #[test]
    fn left_children_sit_left_of_root_right_children_right() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            root: n(
                "Root",
                NodeSide::Default,
                vec![
                    n("L1", NodeSide::Left, vec![]),
                    n("R1", NodeSide::Right, vec![]),
                    // Default goes to right column per v1 rule.
                    n("D1", NodeSide::Default, vec![]),
                ],
            ),
        };
        let s = render(&mm);
        assert_eq!(s.matches("body: node[").count(), 4, "got: {s}");
        assert_eq!(s.matches("(points: (").count(), 3, "got: {s}");

        // Parse each node line's x back out and compare column order.
        let x_of = |label: &str| -> f64 {
            let line = s
                .lines()
                .find(|l| l.contains(&format!("body: node[{label}]")))
                .unwrap_or_else(|| panic!("node {label} missing: {s}"));
            let start = line.find("(x: ").unwrap() + 4;
            let end = line[start..].find("pt").unwrap() + start;
            line[start..end].parse().unwrap()
        };
        assert!(x_of("L1") < x_of("Root"), "left child left of root: {s}");
        assert!(x_of("R1") > x_of("Root"), "right child right of root: {s}");
        assert!(x_of("D1") > x_of("Root"), "default child right of root: {s}");
    }

    #[test]
    fn multilevel_branch_children_extend_outward() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            root: n(
                "Root",
                NodeSide::Default,
                vec![n(
                    "B",
                    NodeSide::Right,
                    vec![n("B1", NodeSide::Default, vec![])],
                )],
            ),
        };
        let s = render(&mm);
        assert_eq!(s.matches("body: node[").count(), 3, "got: {s}");
        // Grandchild further right than its branch root.
        let x_of = |label: &str| -> f64 {
            let line = s
                .lines()
                .find(|l| l.contains(&format!("body: node[{label}]")))
                .unwrap();
            let start = line.find("(x: ").unwrap() + 4;
            let end = line[start..].find("pt").unwrap() + start;
            line[start..end].parse().unwrap()
        };
        assert!(x_of("B1") > x_of("B"), "got: {s}");
    }

    #[test]
    fn left_branch_grandchildren_extend_leftward() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            root: n(
                "Root",
                NodeSide::Default,
                vec![n(
                    "B",
                    NodeSide::Left,
                    vec![n("B1", NodeSide::Default, vec![])],
                )],
            ),
        };
        let s = render(&mm);
        let x_of = |label: &str| -> f64 {
            let line = s
                .lines()
                .find(|l| l.contains(&format!("body: node[{label}]")))
                .unwrap();
            let start = line.find("(x: ").unwrap() + 4;
            let end = line[start..].find("pt").unwrap() + start;
            line[start..end].parse().unwrap()
        };
        assert!(x_of("B1") < x_of("B"), "got: {s}");
        assert!(x_of("B") < x_of("Root"), "got: {s}");
    }
}
