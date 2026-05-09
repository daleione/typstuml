//! WBS diagram codegen.
//!
//! Flattens a [`WbsDiagram`]'s [`TreeNode`] tree into a single nested
//! `tree(node[…], …)` Typst expression, which `vendor/blockcell/src/tree.typ`
//! lays out top-down with elbow connectors. v1 ignores [`NodeSide`] (so all
//! children stack horizontally below their parent) and [`NodeShape::Line`]
//! (rendered as a default `node`) — both fields stay in the IR for the M2
//! direction-aware painter and the M3 underline-shape extension.
//!
//! Color spec parsing here is deliberately narrow: only `#hex` / `#named`
//! forms are recognized, and unknown names degrade silently to the painter's
//! default fill. The shared color-spec parser (roadmap P0.3) will replace
//! this with a fuller mapping.

use crate::ir::{NodeShape, TreeNode, WbsDiagram};

pub fn emit(out: &mut String, wbs: &WbsDiagram) {
    if let Some(title) = &wbs.title {
        out.push_str("#align(center)[*");
        out.push_str(&typst_markup_escape(title));
        out.push_str("*]\n\n");
    }
    out.push_str("#align(center, ");
    emit_subtree(out, &wbs.root, 0);
    out.push_str(")\n");
}

fn emit_subtree(out: &mut String, node: &TreeNode, indent: usize) {
    if node.children.is_empty() {
        emit_node_call(out, node);
        return;
    }
    out.push_str("tree(\n");
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

fn emit_node_call(out: &mut String, node: &TreeNode) {
    let fill_arg = node.fill.as_deref().and_then(typst_color);

    match (&fill_arg, node.shape) {
        (None, NodeShape::Box | NodeShape::Line) => {
            out.push_str("node");
        }
        (Some(fill), NodeShape::Box | NodeShape::Line) => {
            out.push_str("node(fill: ");
            out.push_str(fill);
            out.push(')');
        }
    }

    out.push('[');
    emit_label_body(out, node);
    out.push(']');
}

fn emit_label_body(out: &mut String, node: &TreeNode) {
    let lines: Vec<&str> = if node.label.is_empty() {
        // Empty label: prefer the node's id so the user still sees something.
        // Falls back to a single space so the painter has a non-empty body.
        match node.id.as_deref() {
            Some(id) if !id.is_empty() => vec![id],
            _ => vec![" "],
        }
    } else {
        node.label.iter().map(String::as_str).collect()
    };
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            // Hard line break in Typst markup: backslash followed by newline.
            out.push_str(" \\\n");
        }
        out.push_str(&typst_markup_escape(line));
    }
}

fn indent_spaces(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// Translate a PlantUML `#color` spec to a Typst color expression. Returns
/// `None` for forms we can't safely lower; the caller falls back to the
/// painter's default fill instead of emitting something Typst would reject.
fn typst_color(spec: &str) -> Option<String> {
    let s = spec.strip_prefix('#')?;
    if s.is_empty() {
        return None;
    }
    let is_hex = matches!(s.len(), 3 | 4 | 6 | 8) && s.chars().all(|c| c.is_ascii_hexdigit());
    if is_hex {
        return Some(format!("rgb(\"#{s}\")"));
    }
    // Tiny built-in mapping — Typst only provides a small set of named
    // colors out of the box; anything unrecognised drops to default.
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "black" | "white" | "gray" | "silver" | "red" | "maroon" | "yellow" | "olive" | "lime"
        | "green" | "aqua" | "teal" | "blue" | "navy" | "fuchsia" | "purple" => Some(lower),
        _ => None,
    }
}

fn typst_markup_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '*' | '_' | '#' | '$' | '`' | '~' | '@' | '<' | '>' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{NodeSide, TreeNode};

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
    fn unknown_named_color_falls_back_to_default_fill() {
        let mut root = n("Root", vec![]);
        root.fill = Some("#NotAColor".into());
        let s = render(&WbsDiagram {
            name: None,
            title: None,
            root,
        });
        assert!(!s.contains("fill:"), "should drop unknown color, got: {s}");
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
