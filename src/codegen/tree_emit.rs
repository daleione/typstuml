//! Shared per-node Typst emission used by WBS and mind-map codegen.
//!
//! Both diagrams emit `node[…]` / `node(fill: …, shape: "plain")[…]`
//! calls — as probe bodies in pass-1 and as `tree-layout` node bodies in
//! pass-2 (see [`super::tree_graph`]). This module owns that single-node
//! emission plus the title helper and the narrow color-spec translation.
//!
//! Color specs resolve through the shared [`crate::colors`] table
//! (hex + the full SVG/X11 name set PlantUML accepts). Unknown forms
//! degrade silently to the painter's default fill so we never emit
//! something Typst would reject.

use crate::ir::{NodeShape, TreeNode};

/// Emit `node[…]` or `node(fill: …, shape: "plain")[…]` for a single
/// node, including any decoration arguments.
pub fn emit_node_call(out: &mut String, node: &TreeNode) {
    // Phantoms are never painted — emitters skip them entirely; this
    // arm only exists so a stray call degrades to the boxless form.
    let boxless = matches!(node.shape, NodeShape::Line | NodeShape::Phantom);
    // PlantUML ignores `[#color]` on `_` (boxless) nodes — skip the
    // fill arg so the painter doesn't have to.
    let fill_arg = if boxless {
        None
    } else {
        node.fill.as_deref().and_then(typst_color)
    };

    let mut args: Vec<String> = Vec::new();
    if let Some(fill) = fill_arg {
        args.push(format!("fill: {fill}"));
    }
    if boxless {
        // `_` = "remove the box drawing" → the painter's bare-text shape.
        args.push("shape: \"plain\"".into());
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

/// Translate a PlantUML `#color` spec (hex or SVG/X11 color name) to a
/// Typst color expression via the shared resolver ([`crate::colors`]).
/// Returns `None` for forms we can't safely lower; the caller falls back
/// to the painter's default fill instead of emitting something Typst
/// would reject.
pub fn typst_color(spec: &str) -> Option<String> {
    crate::colors::spec_to_hex(spec).map(|hex| format!("rgb(\"{hex}\")"))
}

pub(crate) use crate::codegen::common::escape_markup as typst_markup_escape;
