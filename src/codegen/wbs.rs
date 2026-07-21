//! WBS diagram codegen.
//!
//! Computes the top-down tree geometry in Rust ([`crate::layout::tree`],
//! a faithful port of `tree.typ`'s down-direction layout) and emits a
//! `#tree-layout(...)` call carrying absolute coordinates. Node sizes
//! come from the pass-1 measure protocol via [`super::tree_graph`],
//! falling back to a heuristic estimator.
//!
//! v1 ignores [`crate::ir::NodeSide`] (children stack horizontally below
//! their parent); `NodeShape::Line` (PlantUML `_`, boxless) maps to `node(shape: "plain")`.

use crate::codegen::tree_emit::emit_title;
use crate::codegen::tree_graph;
use crate::ir::WbsDiagram;
use crate::layout::tree::{layout_wbs, TreeConfig};
use crate::runtime::MeasurementSet;

pub fn emit(
    out: &mut String,
    wbs: &WbsDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    if let Some(title) = &wbs.title {
        emit_title(out, title);
    }

    let em = tree_graph::resolve_em(measurements, diagram_idx);
    let cfg = TreeConfig::from_em(em);
    let mut counter = 0;
    let input = tree_graph::build_input(&wbs.root, &mut counter, measurements, diagram_idx, em);
    let layout = layout_wbs(&input, &cfg);
    let flat = tree_graph::flatten(&wbs.root);

    out.push_str("#align(center, ");
    tree_graph::emit_layout_call(out, &layout, &flat);
    out.push_str(")\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{NodeShape, NodeSide, TreeNode};

    fn n(label: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            label: vec![label.to_string()],
            side: NodeSide::Default,
            shape: NodeShape::Box,
            fill: None,
            id: None,
            line: 0,
            children,
        }
    }

    fn render(wbs: &WbsDiagram) -> String {
        let mut s = String::new();
        emit(&mut s, wbs, None, 0);
        s
    }

    #[test]
    fn leaf_emits_positioned_node() {
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![]),
        });
        assert!(s.contains("tree-layout("), "got: {s}");
        assert!(s.contains("body: node[Root]"), "got: {s}");
        assert!(s.contains("x: 0.000pt, y: 0.000pt"), "got: {s}");
    }

    #[test]
    fn nested_tree_emits_all_nodes_and_edges() {
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![n("A", vec![n("A1", vec![])]), n("B", vec![])]),
        });
        assert_eq!(s.matches("body: node[").count(), 4, "got: {s}");
        // Level-2 elbows (Root→A, Root→B) + A's outline trunk + stub.
        assert_eq!(s.matches("(points: (").count(), 4, "got: {s}");
    }

    #[test]
    fn children_placed_below_root() {
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![n("A", vec![]), n("B", vec![])]),
        });
        // Root at y=0; children share a lower row (root_h + y_gap =
        // 12+8 + 22 = 42pt with the 10pt-em heuristic).
        assert!(s.contains("y: 0.000pt"), "got: {s}");
        let child_rows = s.matches("y: 42.000pt").count();
        assert_eq!(child_rows, 2, "got: {s}");
    }

    #[test]
    fn title_precedes_layout() {
        let s = render(&WbsDiagram {
            name: None,
            title: Some("Org".into()),
            root: n("Root", vec![]),
        });
        assert!(s.starts_with("#align(center)[*Org*]"), "got: {s}");
    }

    #[test]
    fn phantom_node_emits_no_body_but_children_stay() {
        let mut phantom = n(" ", vec![n("E5", vec![]), n("E6", vec![])]);
        phantom.shape = NodeShape::Phantom;
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![phantom]),
        });
        // Root + E5 + E6 painted; the phantom itself is not.
        assert_eq!(s.matches("body: node[").count(), 3, "got: {s}");
        assert!(s.contains("node[E5]"), "got: {s}");
        // Structure edges still flow through it: root elbow + trunk + 2 stubs.
        assert_eq!(s.matches("(points: (").count(), 4, "got: {s}");
    }

    #[test]
    fn boxless_and_fill_decorations_survive() {
        let mut colored = n("C", vec![]);
        colored.fill = Some("#FF0000".into());
        let mut boxless = n("L", vec![]);
        boxless.shape = NodeShape::Line;
        // A fill on a boxless node is ignored (PlantUML semantics).
        boxless.fill = Some("#00FF00".into());
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![colored, boxless]),
        });
        assert!(s.contains("node(fill: rgb(\"#FF0000\"))[C]"), "got: {s}");
        assert!(s.contains("node(shape: \"plain\")[L]"), "got: {s}");
        assert!(!s.contains("#00FF00"), "boxless fill must be dropped: {s}");
    }
}
