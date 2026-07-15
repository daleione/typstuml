//! WBS diagram codegen.
//!
//! Flattens a [`WbsDiagram`]'s tree into a single `tree(node[…], …)` Typst
//! expression rendered top-down by `components/src/tree.typ`. v1
//! ignores `NodeSide` (children stack horizontally below their parent);
//! `NodeShape::Line` maps to `node(shape: "underline")`. The recursive
//! emission and decoration plumbing live in [`super::tree_emit`].

use crate::codegen::tree_emit::{emit_subtree, emit_title};
use crate::ir::WbsDiagram;

pub fn emit(out: &mut String, wbs: &WbsDiagram) {
    if let Some(title) = &wbs.title {
        emit_title(out, title);
    }
    out.push_str("#align(center, ");
    emit_subtree(out, &wbs.root, 0);
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
        emit(&mut s, wbs);
        s
    }

    #[test]
    fn leaf_emits_bare_node() {
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![]),
        });
        assert!(s.contains("node[Root]"), "got: {s}");
    }

    #[test]
    fn nested_tree_uses_tree_constructor() {
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root: n("Root", vec![n("A", vec![n("A1", vec![])]), n("B", vec![])]),
        });
        assert!(s.contains("tree("), "expected tree() call, got: {s}");
        assert!(s.contains("node[A1]"));
        assert!(s.contains("node[B]"));
    }

    #[test]
    fn hex_color_becomes_rgb_call() {
        let mut root = n("Root", vec![]);
        root.fill = Some("#FF0000".into());
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root,
        });
        assert!(s.contains("rgb(\"#FF0000\")"), "got: {s}");
    }

    #[test]
    fn line_shape_maps_to_underline() {
        let mut root = n("Bare", vec![]);
        root.shape = NodeShape::Line;
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root,
        });
        assert!(s.contains("shape: \"underline\""), "got: {s}");
    }

    #[test]
    fn multiline_label_uses_typst_linebreak() {
        let mut root = n("Root", vec![]);
        root.label = vec!["line one".into(), "line two".into()];
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root,
        });
        assert!(s.contains("line one \\\nline two"), "got: {s}");
    }

    #[test]
    fn typst_specials_are_escaped() {
        let root = n("a*b_c#d", vec![]);
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root,
        });
        assert!(s.contains("a\\*b\\_c\\#d"), "got: {s}");
    }

    #[test]
    fn title_emits_centered_bold_block() {
        let s = render(&WbsDiagram {
            name: None,
            title: Some("Org chart".into()),
            root: n("CEO", vec![]),
        });
        assert!(s.contains("#align(center)[*Org chart*]"));
    }
}
