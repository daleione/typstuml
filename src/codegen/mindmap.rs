//! Mind-map diagram codegen.
//!
//! For every root in the diagram (PlantUML allows several — they stack),
//! splits the first-level children by [`NodeSide`], computes the
//! two-column mind-map geometry in Rust ([`crate::layout::tree`]) and
//! emits a single `#tree-layout(...)` call with absolute coordinates.
//! Node sizes come from the pass-1 measure protocol via
//! [`super::tree_graph`].
//!
//! `top to bottom direction` uses the transpose trick: node sizes are
//! swapped, the ordinary left-right layout runs, and the finished
//! layout transposes — left-side branches end up growing upward,
//! right-side downward, exactly PlantUML's rendering.
//!
//! v1 simplification: a child whose parsed side is `Default` (no
//! explicit `+`/`-` and no `left side` directive) is treated as
//! right-side. PlantUML's auto-balance heuristic is left for a
//! follow-up.

use crate::codegen::tree_emit::emit_title;
use crate::codegen::tree_graph;
use crate::ir::{MapDirection, MindMapDiagram, NodeSide};
use crate::layout::tree::{
    layout_mindmap, stack_layouts, transpose_layout, TreeConfig, TreeLayoutInput,
};
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
    let ttb = mm.direction == MapDirection::TopToBottom;

    // IDs are assigned in pre-order across roots in source order (the
    // same order `flatten_forest` / `collect_probes` walk); the side
    // partition below only affects which column a subtree lands in.
    let mut counter = 0;
    let mut layouts = Vec::new();
    for root in &mm.roots {
        let root_id = counter;
        counter += 1;
        let root_size =
            tree_graph::resolve_node_size(root, root_id, measurements, diagram_idx, em);
        let mut root_input = TreeLayoutInput {
            id: root_id,
            size: root_size,
            side: tree_graph::layout_side(root.side),
            children: Vec::new(),
        };

        let mut lefts: Vec<TreeLayoutInput> = Vec::new();
        let mut rights: Vec<TreeLayoutInput> = Vec::new();
        for child in &root.children {
            let mut input =
                tree_graph::build_input(child, &mut counter, measurements, diagram_idx, em);
            if ttb {
                tree_graph::swap_input_sizes(&mut input);
            }
            match child.side {
                NodeSide::Left => lefts.push(input),
                // Right and Default both map to the right column in v1.
                NodeSide::Right | NodeSide::Default => rights.push(input),
            }
        }
        if ttb {
            tree_graph::swap_input_sizes(&mut root_input);
        }

        let layout = layout_mindmap(&root_input, &lefts, &rights, &cfg);
        layouts.push(if ttb { transpose_layout(layout) } else { layout });
    }

    // Multi-root: left-right maps stack vertically (PlantUML's
    // rendering); top-to-bottom maps stack horizontally.
    let layout = stack_layouts(layouts, cfg.y_gap, ttb);
    let flat = tree_graph::flatten_forest(&mm.roots);

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

    fn single(root: TreeNode) -> MindMapDiagram {
        MindMapDiagram {
            name: None,
            title: None,
            roots: vec![root],
            direction: MapDirection::LeftToRight,
        }
    }

    fn render(mm: &MindMapDiagram) -> String {
        let mut s = String::new();
        emit(&mut s, mm, None, 0);
        s
    }

    fn coord_of(s: &str, label: &str, key: &str) -> f64 {
        let line = s
            .lines()
            .find(|l| l.contains(&format!("body: node[{label}]")))
            .unwrap_or_else(|| panic!("node {label} missing: {s}"));
        let pat = format!("{key}: ");
        let start = line.find(&pat).unwrap() + pat.len();
        let end = line[start..].find("pt").unwrap() + start;
        line[start..end].parse().unwrap()
    }

    #[test]
    fn root_only_emits_single_node_layout() {
        let s = render(&single(n("Root", NodeSide::Default, vec![])));
        assert!(s.contains("tree-layout("), "got: {s}");
        assert_eq!(s.matches("body: node[").count(), 1, "got: {s}");
        assert_eq!(s.matches("(points: (").count(), 0, "got: {s}");
    }

    #[test]
    fn left_children_sit_left_of_root_right_children_right() {
        let mm = single(n(
            "Root",
            NodeSide::Default,
            vec![
                n("L1", NodeSide::Left, vec![]),
                n("R1", NodeSide::Right, vec![]),
                // Default goes to right column per v1 rule.
                n("D1", NodeSide::Default, vec![]),
            ],
        ));
        let s = render(&mm);
        assert_eq!(s.matches("body: node[").count(), 4, "got: {s}");
        assert_eq!(s.matches("(points: (").count(), 3, "got: {s}");
        assert!(coord_of(&s, "L1", "x") < coord_of(&s, "Root", "x"), "got: {s}");
        assert!(coord_of(&s, "R1", "x") > coord_of(&s, "Root", "x"), "got: {s}");
        assert!(coord_of(&s, "D1", "x") > coord_of(&s, "Root", "x"), "got: {s}");
    }

    #[test]
    fn multiroot_stacks_vertically() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            roots: vec![
                n("Root 1", NodeSide::Default, vec![n("Foo", NodeSide::Default, vec![])]),
                n("Root 2", NodeSide::Default, vec![n("Lorem", NodeSide::Default, vec![])]),
            ],
            direction: MapDirection::LeftToRight,
        };
        let s = render(&mm);
        assert_eq!(s.matches("body: node[").count(), 4, "got: {s}");
        assert!(
            coord_of(&s, "Root 2", "y") > coord_of(&s, "Root 1", "y"),
            "second map below first: {s}"
        );
        assert!(
            coord_of(&s, "Root 2", "y") > coord_of(&s, "Foo", "y"),
            "second map clears first map's content: {s}"
        );
    }

    #[test]
    fn top_to_bottom_transposes() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            roots: vec![n(
                "1",
                NodeSide::Default,
                vec![
                    n("up", NodeSide::Left, vec![]),
                    n("down", NodeSide::Right, vec![]),
                ],
            )],
            direction: MapDirection::TopToBottom,
        };
        let s = render(&mm);
        // Left side grows up, right side grows down; all share x-ish.
        assert!(coord_of(&s, "up", "y") < coord_of(&s, "1", "y"), "got: {s}");
        assert!(coord_of(&s, "down", "y") > coord_of(&s, "1", "y"), "got: {s}");
    }

    #[test]
    fn multilevel_branch_children_extend_outward() {
        let mm = single(n(
            "Root",
            NodeSide::Default,
            vec![n(
                "B",
                NodeSide::Right,
                vec![n("B1", NodeSide::Default, vec![])],
            )],
        ));
        let s = render(&mm);
        assert_eq!(s.matches("body: node[").count(), 3, "got: {s}");
        assert!(coord_of(&s, "B1", "x") > coord_of(&s, "B", "x"), "got: {s}");
    }

    #[test]
    fn left_branch_grandchildren_extend_leftward() {
        let mm = single(n(
            "Root",
            NodeSide::Default,
            vec![n(
                "B",
                NodeSide::Left,
                vec![n("B1", NodeSide::Default, vec![])],
            )],
        ));
        let s = render(&mm);
        assert!(coord_of(&s, "B1", "x") < coord_of(&s, "B", "x"), "got: {s}");
        assert!(coord_of(&s, "B", "x") < coord_of(&s, "Root", "x"), "got: {s}");
    }
}
