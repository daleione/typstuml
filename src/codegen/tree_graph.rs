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
use crate::ir::{NodeSide, TreeNode};
use crate::layout::tree::{Side, TreeLayout, TreeLayoutInput};
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

/// Emit pass-1 probes for every node of a forest, plus the em probe.
/// IDs run pre-order across the roots in order — the same contract the
/// emitters and the web model use.
pub(crate) fn collect_probes(
    roots: &[TreeNode],
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    let em_id = tree_em_id(diagram_idx);
    out.push_str("#tree-em-probe(id: \"");
    out.push_str(&em_id);
    out.push_str("\")\n");
    expected_ids.push(em_id);

    for (i, node) in flatten_forest(roots).iter().enumerate() {
        // Phantom nodes (WBS layer-skipping) have no rendered content —
        // no probe; their size is pinned to zero at layout time.
        if node.shape == crate::ir::NodeShape::Phantom {
            continue;
        }
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

/// [`flatten`] across every root in order.
pub(crate) fn flatten_forest(roots: &[TreeNode]) -> Vec<&TreeNode> {
    roots.iter().flat_map(flatten).collect()
}

/// Recursively swap every node's (w, h) — feeds the transpose trick for
/// `top to bottom direction` mind maps.
pub(crate) fn swap_input_sizes(input: &mut TreeLayoutInput) {
    input.size = (input.size.1, input.size.0);
    for c in &mut input.children {
        swap_input_sizes(c);
    }
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
    if node.shape == crate::ir::NodeShape::Phantom {
        // Removed node: occupies no space; its outline trunk drops
        // straight from where it would have been.
        return (0.0, 0.0);
    }
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
    TreeLayoutInput {
        id,
        size,
        side: layout_side(node.side),
        children,
    }
}

/// IR side → layout side. PlantUML's `<` marker hangs a WBS outline
/// node on the left; `>` and unmarked hang right.
pub(crate) fn layout_side(side: NodeSide) -> Side {
    match side {
        NodeSide::Left => Side::Left,
        NodeSide::Right | NodeSide::Default => Side::Right,
    }
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
        // Phantom nodes are pure structure — nothing to paint.
        if flat[n.id].shape == crate::ir::NodeShape::Phantom {
            continue;
        }
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

    out.push(')');
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
        let roots = vec![n("R", vec![n("A", vec![])]), n("R2", vec![])];
        let mut out = String::new();
        let mut ids = Vec::new();
        collect_probes(&roots, 2, &mut out, &mut ids);
        // Pre-order across roots: R, A, R2.
        assert_eq!(ids, vec!["te-2", "tn-2-0", "tn-2-1", "tn-2-2"]);
        assert!(out.contains("#tree-em-probe(id: \"te-2\")"));
        assert!(out.contains("#tree-probe(id: \"tn-2-1\", node[A])"));
        assert!(out.contains("#tree-probe(id: \"tn-2-2\", node[R2])"));
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
