//! Shared `#tree-layout(...)` emitter used by WBS and mind-map codegen.
//!
//! Mirrors the `record_graph` architecture: node sizes come from the
//! pass-1 measure protocol (`#tree-probe` / `#tree-em-probe`, falling
//! back to a heuristic estimator), the geometry is computed in Rust by
//! [`crate::layout::tree`] — a faithful port of the old Typst-side
//! layout — and the emitted `#tree-layout(...)` call carries absolute
//! coordinates that the Typst painter consumes verbatim.
//!
//! Node identity: pre-order index over the IR tree (root = 0, then each
//! child subtree in source order). Probe IDs are `tn-<diagram>-<node>`;
//! the per-diagram `te-<diagram>` probe reports the resolved size of
//! `1em`, from which every gap constant is derived.

use crate::codegen::tree_emit::emit_node_call;
use crate::ir::TreeNode;
use crate::layout::tree::{TreeLayout, TreeLayoutInput};
use crate::runtime::MeasurementSet;

/// Fallback font size when no measurement set is available. Matches the
/// `#set text(size: 10pt)` preamble.
const FALLBACK_EM_PT: f64 = 10.0;
/// Heuristic per-glyph width in em: conservative ASCII estimate; CJK /
/// emoji counted as a full em (same scheme as `record_graph`).
const GLYPH_EM: f64 = 0.55;
/// Heuristic line height (Typst default leading at body size).
const LINE_HEIGHT_EM: f64 = 1.2;
/// `node()` insets: `(x: 0.8em, y: 0.4em)`.
const INSET_X_EM: f64 = 0.8;
const INSET_Y_EM: f64 = 0.4;

/// Stable per-node probe ID for the measure protocol.
pub(crate) fn tree_node_id(diagram_idx: usize, node_idx: usize) -> String {
    format!("tn-{diagram_idx}-{node_idx}")
}

/// Per-diagram probe ID reporting the resolved `1em`.
pub(crate) fn tree_em_id(diagram_idx: usize) -> String {
    format!("te-{diagram_idx}")
}

/// Emit pass-1 probes for every node of `root`'s tree, plus the em
/// probe. IDs and order match what the emitters consume.
pub(crate) fn collect_probes(
    root: &TreeNode,
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    let em_id = tree_em_id(diagram_idx);
    out.push_str("#tree-em-probe(id: \"");
    out.push_str(&em_id);
    out.push_str("\")\n");
    expected_ids.push(em_id);

    let flat = flatten(root);
    for (i, node) in flat.iter().enumerate() {
        let id = tree_node_id(diagram_idx, i);
        out.push_str("#tree-probe(id: \"");
        out.push_str(&id);
        out.push_str("\", ");
        emit_node_call(out, node);
        out.push_str(")\n");
        expected_ids.push(id);
    }
}

/// Pre-order flattening — the ID ↔ node contract shared by probes,
/// layout input, and emission.
pub(crate) fn flatten(root: &TreeNode) -> Vec<&TreeNode> {
    let mut flat = Vec::new();
    fn walk<'a>(n: &'a TreeNode, flat: &mut Vec<&'a TreeNode>) {
        flat.push(n);
        for c in &n.children {
            walk(c, flat);
        }
    }
    walk(root, &mut flat);
    flat
}

/// Resolved `1em` in pt: measured when available, `10pt` otherwise.
pub(crate) fn resolve_em(measurements: Option<&MeasurementSet>, diagram_idx: usize) -> f64 {
    measurements
        .and_then(|set| set.get(&tree_em_id(diagram_idx)))
        .map(|m| m.width_pt)
        .unwrap_or(FALLBACK_EM_PT)
}

/// Node size: pass-1 measurement when available, heuristic otherwise.
/// Falls back per-node so a partial measurement set still gets most
/// nodes right.
pub(crate) fn resolve_node_size(
    node: &TreeNode,
    node_idx: usize,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
    em: f64,
) -> (f64, f64) {
    if let Some(set) = measurements {
        if let Some(m) = set.get(&tree_node_id(diagram_idx, node_idx)) {
            return (m.width_pt, m.height_pt);
        }
    }
    heuristic_node_size(node, em)
}

/// Rough `node(...)` bbox: max line width + x-insets, line count ×
/// leading + y-insets. Only used when the measure pass failed — the
/// painter centers the natural-size body on this slot, so being close
/// is enough.
fn heuristic_node_size(node: &TreeNode, em: f64) -> (f64, f64) {
    let lines: Vec<&str> = if node.label.is_empty() {
        match node.id.as_deref() {
            Some(id) if !id.is_empty() => vec![id],
            _ => vec![" "],
        }
    } else {
        node.label.iter().map(String::as_str).collect()
    };
    let text_w = lines
        .iter()
        .map(|l| {
            l.chars()
                .map(|c| if c.is_ascii() { GLYPH_EM * em } else { em })
                .sum::<f64>()
        })
        .fold(0.0, f64::max);
    let text_h = lines.len() as f64 * LINE_HEIGHT_EM * em;
    (
        text_w + 2.0 * INSET_X_EM * em,
        text_h + 2.0 * INSET_Y_EM * em,
    )
}

/// Build the layout input for `node`'s subtree, assigning pre-order IDs
/// from `counter` (must walk the same order as [`flatten`]).
pub(crate) fn build_input(
    node: &TreeNode,
    counter: &mut usize,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
    em: f64,
) -> TreeLayoutInput {
    let id = *counter;
    *counter += 1;
    let size = resolve_node_size(node, id, measurements, diagram_idx, em);
    let children = node
        .children
        .iter()
        .map(|c| build_input(c, counter, measurements, diagram_idx, em))
        .collect();
    TreeLayoutInput { id, size, children }
}

/// Emit the `#tree-layout(...)` call for a finished layout. `flat` maps
/// node IDs back to their IR nodes for body emission. The caller wraps
/// with `#align(center, …)` and the closing `)`.
pub(crate) fn emit_layout_call(out: &mut String, layout: &TreeLayout, flat: &[&TreeNode]) {
    out.push_str(&format!(
        "tree-layout(\n  width: {:.3}pt,\n  height: {:.3}pt,\n",
        layout.width, layout.height
    ));

    out.push_str("  nodes: (\n");
    // Emit in ID order so goldens read in source pre-order regardless of
    // the order layout merged blobs in.
    let mut nodes = layout.nodes.clone();
    nodes.sort_by_key(|n| n.id);
    for n in &nodes {
        out.push_str(&format!(
            "    (x: {:.3}pt, y: {:.3}pt, w: {:.3}pt, h: {:.3}pt, body: ",
            n.x, n.y, n.w, n.h
        ));
        emit_node_call(out, flat[n.id]);
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    out.push_str("  edges: (\n");
    for e in &layout.edges {
        out.push_str("    (points: (");
        for (i, p) in e.points.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("({:.3}pt, {:.3}pt)", p.0, p.1));
        }
        // Trailing comma: points always has ≥ 2 entries, but keep the
        // tuple form explicit anyway.
        out.push_str(",),),\n");
    }
    out.push_str("  ),\n");

    out.push_str(")");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{NodeShape, NodeSide};

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

    #[test]
    fn flatten_is_preorder() {
        let root = n("R", vec![n("A", vec![n("A1", vec![])]), n("B", vec![])]);
        let flat = flatten(&root);
        let labels: Vec<&str> = flat.iter().map(|t| t.label[0].as_str()).collect();
        assert_eq!(labels, vec!["R", "A", "A1", "B"]);
    }

    #[test]
    fn build_input_ids_match_flatten_order() {
        let root = n("R", vec![n("A", vec![n("A1", vec![])]), n("B", vec![])]);
        let mut counter = 0;
        let input = build_input(&root, &mut counter, None, 0, 10.0);
        assert_eq!(input.id, 0);
        assert_eq!(input.children[0].id, 1);
        assert_eq!(input.children[0].children[0].id, 2);
        assert_eq!(input.children[1].id, 3);
        assert_eq!(counter, 4);
    }

    #[test]
    fn probes_cover_every_node_plus_em() {
        let root = n("R", vec![n("A", vec![]), n("B", vec![])]);
        let mut out = String::new();
        let mut ids = Vec::new();
        collect_probes(&root, 2, &mut out, &mut ids);
        assert_eq!(ids, vec!["te-2", "tn-2-0", "tn-2-1", "tn-2-2"]);
        assert!(out.contains("#tree-em-probe(id: \"te-2\")"));
        assert!(out.contains("#tree-probe(id: \"tn-2-1\", node[A])"));
    }

    #[test]
    fn heuristic_size_scales_with_em() {
        let node = n("ab", vec![]);
        let (w10, h10) = heuristic_node_size(&node, 10.0);
        let (w20, h20) = heuristic_node_size(&node, 20.0);
        assert!((w20 - 2.0 * w10).abs() < 1e-9);
        assert!((h20 - 2.0 * h10).abs() < 1e-9);
    }
}
