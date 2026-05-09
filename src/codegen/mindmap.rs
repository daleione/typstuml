//! Mind-map diagram codegen.
//!
//! Splits the root's first-level children by [`NodeSide`] and emits
//!
//! ```text
//! mindmap(
//!   <root node>,
//!   lefts:  ( <subtree>, <subtree>, … ),
//!   rights: ( <subtree>, <subtree>, … ),
//! )
//! ```
//!
//! Each first-level subtree is wrapped in `tree(direction: "left"|"right",
//! …)`; deeper levels rely on `tree.typ`'s direction-state inheritance and
//! drop the `direction:` arg.
//!
//! v1 simplification (per `docs/mindmap-wbs-plan.md` §3.2): a child whose
//! parsed side is `Default` (no explicit `+`/`-`, e.g. plain `**` markers)
//! is treated as right-side. PlantUML's auto-balance heuristic is left for
//! a follow-up.

use crate::codegen::tree_emit::{emit_node_call, emit_subtree, emit_title, indent_spaces};
use crate::ir::{MindMapDiagram, NodeSide, TreeNode};

pub fn emit(out: &mut String, mm: &MindMapDiagram) {
    if let Some(title) = &mm.title {
        emit_title(out, title);
    }

    let (lefts, rights) = partition_children(&mm.root.children);

    out.push_str("#align(center, mindmap(\n");
    indent_spaces(out, 1);
    emit_node_call(out, &mm.root);
    out.push_str(",\n");

    indent_spaces(out, 1);
    out.push_str("lefts: (\n");
    for child in &lefts {
        indent_spaces(out, 2);
        emit_branch(out, child, "left", 2);
        out.push_str(",\n");
    }
    indent_spaces(out, 1);
    out.push_str("),\n");

    indent_spaces(out, 1);
    out.push_str("rights: (\n");
    for child in &rights {
        indent_spaces(out, 2);
        emit_branch(out, child, "right", 2);
        out.push_str(",\n");
    }
    indent_spaces(out, 1);
    out.push_str("),\n");

    out.push_str("))\n");
}

/// Wrap a first-level subtree in `tree(direction: "left|right", …)` so its
/// root anchor lands on the inner edge facing `mindmap`. Leaves with no
/// children are emitted bare — they have no internal layout for a direction
/// to influence, so the wrapper would just add noise.
fn emit_branch(out: &mut String, node: &TreeNode, direction: &str, indent: usize) {
    if node.children.is_empty() {
        emit_node_call(out, node);
        return;
    }
    out.push_str("tree(direction: \"");
    out.push_str(direction);
    out.push_str("\",\n");
    indent_spaces(out, indent + 1);
    emit_node_call(out, node);
    out.push_str(",\n");
    for child in &node.children {
        indent_spaces(out, indent + 1);
        emit_subtree(out, child, indent + 1);
        out.push_str(",\n");
    }
    indent_spaces(out, indent);
    out.push(')');
}

fn partition_children(children: &[TreeNode]) -> (Vec<&TreeNode>, Vec<&TreeNode>) {
    let mut lefts = Vec::new();
    let mut rights = Vec::new();
    for c in children {
        match c.side {
            NodeSide::Left => lefts.push(c),
            // Right and Default both map to the right column in v1. PlantUML
            // auto-balances `*`-form nodes; we simplify to "all right" until
            // someone produces a fixture where it matters.
            NodeSide::Right | NodeSide::Default => rights.push(c),
        }
    }
    (lefts, rights)
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
        emit(&mut s, mm);
        s
    }

    #[test]
    fn root_only_emits_empty_branches() {
        let s = render(&MindMapDiagram {
            name: None,
            title: None,
            root: n("Root", NodeSide::Default, vec![]),
        });
        assert!(s.contains("mindmap("));
        assert!(s.contains("lefts: (\n"));
        assert!(s.contains("rights: (\n"));
    }

    #[test]
    fn classifies_children_by_side() {
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
        let left_block = &s[s.find("lefts: (").unwrap()..s.find("),\n  rights: (").unwrap()];
        let right_block = &s[s.find("rights: (").unwrap()..];
        assert!(left_block.contains("node[L1]"), "left block: {left_block}");
        assert!(!left_block.contains("node[R1]"));
        assert!(right_block.contains("node[R1]"));
        assert!(right_block.contains("node[D1]"));
    }

    #[test]
    fn multilevel_branch_emits_directional_outer_tree() {
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
        assert!(
            s.contains("tree(direction: \"right\","),
            "expected directional outer wrap: {s}"
        );
        assert!(s.contains("node[B1]"));
    }

    #[test]
    fn leaf_branch_emits_bare_node_no_wrapper() {
        let mm = MindMapDiagram {
            name: None,
            title: None,
            root: n(
                "Root",
                NodeSide::Default,
                vec![n("Solo", NodeSide::Right, vec![])],
            ),
        };
        let s = render(&mm);
        // No tree(direction: …) wrapper around the leaf.
        let right_block = &s[s.find("rights: (").unwrap()..];
        assert!(!right_block.contains("tree(direction:"), "got: {right_block}");
        assert!(right_block.contains("node[Solo]"));
    }

    #[test]
    fn deeper_levels_skip_explicit_direction() {
        // Three levels deep. Only the FIRST level of children carries the
        // direction wrapper; the inner tree() call inherits direction via
        // tree.typ's _tree-direction state.
        let mm = MindMapDiagram {
            name: None,
            title: None,
            root: n(
                "Root",
                NodeSide::Default,
                vec![n(
                    "B",
                    NodeSide::Right,
                    vec![n(
                        "B1",
                        NodeSide::Default,
                        vec![n("B1a", NodeSide::Default, vec![])],
                    )],
                )],
            ),
        };
        let s = render(&mm);
        let direction_count = s.matches("direction:").count();
        assert_eq!(
            direction_count, 1,
            "should only emit direction once (outer wrap), got {direction_count}: {s}"
        );
    }
}
