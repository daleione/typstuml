//! Shared Typst-tree emission used by WBS and mind-map codegen.
//!
//! Both diagrams flatten an IR [`TreeNode`] into a Typst expression tree
//! built from `tree(node[…], …)` and bare `node[…]` calls. The wrappers
//! around that expression differ — WBS just centers the whole tree,
//! mind-map splits the root's first level into two columns and wraps with
//! `mindmap` — so the wrappers stay in their own modules and call
//! into here for the per-node emission.
//!
//! Color spec parsing here is deliberately narrow: `#hex` and a tiny set
//! of Typst-built-in named colors. Unknown forms degrade silently to the
//! painter's default fill so we never emit something Typst would reject.
//! The shared color-spec parser (roadmap P0.3) will replace this with a
//! fuller mapping.

use crate::ir::{NodeShape, TreeNode};

/// Emit a `node[…]` for a leaf or `tree(node[…], child1, child2, …)` for an
/// internal node. `indent` is the column the opening `tree(` sits at; the
/// emitted block uses `indent + 1` for its body.
pub fn emit_subtree(out: &mut String, node: &TreeNode, indent: usize) {
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

/// Emit `node[…]` or `node(fill: …, shape: "underline")[…]` for a single
/// node, including any decoration arguments.
pub fn emit_node_call(out: &mut String, node: &TreeNode) {
    let fill_arg = node.fill.as_deref().and_then(typst_color);
    let needs_underline = matches!(node.shape, NodeShape::Line);

    let mut args: Vec<String> = Vec::new();
    if let Some(fill) = fill_arg {
        args.push(format!("fill: {fill}"));
    }
    if needs_underline {
        args.push("shape: \"underline\"".into());
    }

    if args.is_empty() {
        out.push_str("node");
    } else {
        out.push_str("node(");
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(a);
        }
        out.push(')');
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

/// Emit a `#align(center)[*<title>*]` block followed by a blank line.
pub fn emit_title(out: &mut String, title: &str) {
    out.push_str("#align(center)[*");
    out.push_str(&typst_markup_escape(title));
    out.push_str("*]\n\n");
}

pub fn indent_spaces(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// Translate a PlantUML `#color` spec to a Typst color expression. Returns
/// `None` for forms we can't safely lower; the caller falls back to the
/// painter's default fill instead of emitting something Typst would reject.
pub fn typst_color(spec: &str) -> Option<String> {
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

pub fn typst_markup_escape(s: &str) -> String {
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
