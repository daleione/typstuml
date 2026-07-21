//! Tree / mind-map model and display-list JSON for the web renderer.
//!
//! Model shape (per node):
//!
//! ```json
//! { "id": 0, "label": ["line1"], "shape": "rect" | "underline",
//!   "fill": "#RRGGBB" | null, "side": "left" | "right" | "default",
//!   "children": [ … ] }
//! ```
//!
//! IDs are the pre-order index — the same contract as
//! [`crate::codegen`]'s probe IDs, so a node keeps its identity across
//! CLI and web paths.
//!
//! Display list:
//!
//! ```json
//! { "width": w, "height": h,
//!   "nodes": [ { "id": 0, "x": …, "y": …, "w": …, "h": … } ],
//!   "edges": [ { "from": 0, "to": 1, "points": [[x, y], …] } ] }
//! ```
//!
//! Styling stays in the model; the display list is geometry only. The
//! JS renderer joins the two by node ID.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::diagnostics::CompatMode;
use crate::ir::{Diagram, NodeShape, NodeSide, TreeNode};
use crate::layout::tree::{layout_mindmap, layout_wbs, TreeConfig, TreeLayoutInput};

/// Parse `source` and build the model JSON for its first WBS / mind-map
/// diagram. Errors mirror the render path's diagnostics.
pub fn model_json(source: &str) -> Result<String, String> {
    let config = crate::parser::Config::default();
    let parsed = crate::parser::parse(source, CompatMode::Warn, &config)
        .map_err(|e| e.to_string())?;

    for diagram in &parsed.document.diagrams {
        let (kind, title, root) = match diagram {
            Diagram::Wbs(w) => ("wbs", &w.title, &w.root),
            Diagram::MindMap(m) => ("mindmap", &m.title, &m.root),
            _ => continue,
        };
        let mut counter = 0;
        let model = json!({
            "kind": kind,
            "title": title,
            "root": node_json(root, &mut counter),
        });
        return serde_json::to_string(&model).map_err(|e| e.to_string());
    }
    Err("no mindmap or WBS diagram found in input".into())
}

fn node_json(node: &TreeNode, counter: &mut usize) -> Value {
    let id = *counter;
    *counter += 1;
    let children: Vec<Value> = node.children.iter().map(|c| node_json(c, counter)).collect();
    json!({
        "id": id,
        "label": label_lines(node),
        "shape": match node.shape {
            NodeShape::Box => "rect",
            NodeShape::Line => "underline",
        },
        "fill": node.fill.as_deref().and_then(css_color),
        "side": match node.side {
            NodeSide::Left => "left",
            NodeSide::Right => "right",
            NodeSide::Default => "default",
        },
        "children": children,
    })
}

/// Same empty-label fallback as the Typst emitter (`tree_emit`).
fn label_lines(node: &TreeNode) -> Vec<String> {
    if node.label.is_empty() {
        match node.id.as_deref() {
            Some(id) if !id.is_empty() => vec![id.to_string()],
            _ => vec![" ".to_string()],
        }
    } else {
        node.label.clone()
    }
}

/// Translate a PlantUML `#color` spec to a CSS color. Mirrors
/// `tree_emit::typst_color`'s accepted forms — same hex passthrough,
/// same 16 named colors — but resolves the names to the hex values
/// Typst's built-in color constants use, so both renderers show the
/// same paint. Unknown forms return `None` (painter default fill),
/// matching the CLI's silent-fallback behaviour.
fn css_color(spec: &str) -> Option<String> {
    let s = spec.strip_prefix('#')?;
    if s.is_empty() {
        return None;
    }
    let is_hex = matches!(s.len(), 3 | 4 | 6 | 8) && s.chars().all(|c| c.is_ascii_hexdigit());
    if is_hex {
        return Some(format!("#{s}"));
    }
    // Typst built-in color constants (not CSS named colors — Typst's
    // `red` is #FF4136, CSS's is #FF0000).
    let hex = match s.to_ascii_lowercase().as_str() {
        "black" => "#000000",
        "white" => "#FFFFFF",
        "gray" => "#AAAAAA",
        "silver" => "#DDDDDD",
        "red" => "#FF4136",
        "maroon" => "#85144B",
        "yellow" => "#FFDC00",
        "olive" => "#3D9970",
        "lime" => "#01FF70",
        "green" => "#2ECC40",
        "aqua" => "#7FDBFF",
        "teal" => "#39CCCC",
        "blue" => "#0074D9",
        "navy" => "#001F3F",
        "fuchsia" => "#F012BE",
        "purple" => "#B10DC9",
        _ => return None,
    };
    Some(hex.to_string())
}

/// Compute the display list for `model_json` (as produced by
/// [`model_json`]) given measured node sizes, the set of folded node
/// IDs, and the resolved `em` (the renderer's font size in px — every
/// gap constant scales from it, mirroring the Typst path's em probe).
///
/// `sizes_json`: `{"<id>": [w, h], …}`. A missing entry falls back to a
/// crude estimate so a half-measured tree still lays out.
/// `folded_json`: `[id, …]` — children of these nodes are pruned before
/// layout (their sizes need not be present).
pub fn display_list_json(
    model_json: &str,
    sizes_json: &str,
    folded_json: &str,
    em: f64,
) -> Result<String, String> {
    let model: Value =
        serde_json::from_str(model_json).map_err(|e| format!("model: {e}"))?;
    let sizes: HashMap<String, (f64, f64)> =
        serde_json::from_str(sizes_json).map_err(|e| format!("sizes: {e}"))?;
    let folded: HashSet<usize> =
        serde_json::from_str::<Vec<usize>>(folded_json)
            .map_err(|e| format!("folded: {e}"))?
            .into_iter()
            .collect();
    if !(em.is_finite() && em > 0.0) {
        return Err(format!("em must be positive and finite, got {em}"));
    }

    let kind = model
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("model: missing kind")?
        .to_string();
    let root = model.get("root").ok_or("model: missing root")?;

    let cfg = TreeConfig::from_em(em);
    let layout = match kind.as_str() {
        "wbs" => {
            let input = build_input(root, &sizes, &folded, em)?;
            layout_wbs(&input, &cfg)
        }
        "mindmap" => {
            let root_input = build_input_shallow(root, &sizes, em)?;
            let mut lefts = Vec::new();
            let mut rights = Vec::new();
            if !folded.contains(&root_input.id) {
                for child in child_array(root)? {
                    let side = child.get("side").and_then(Value::as_str).unwrap_or("default");
                    let input = build_input(child, &sizes, &folded, em)?;
                    match side {
                        "left" => lefts.push(input),
                        _ => rights.push(input),
                    }
                }
            }
            layout_mindmap(&root_input, &lefts, &rights, &cfg)
        }
        other => return Err(format!("model: unknown kind {other:?}")),
    };

    let nodes: Vec<Value> = {
        let mut ns = layout.nodes.clone();
        ns.sort_by_key(|n| n.id);
        ns.iter()
            .map(|n| json!({ "id": n.id, "x": n.x, "y": n.y, "w": n.w, "h": n.h }))
            .collect()
    };
    let edges: Vec<Value> = layout
        .edges
        .iter()
        .map(|e| {
            json!({
                "from": e.from,
                "to": e.to,
                "points": e.points.iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::to_string(&json!({
        "width": layout.width,
        "height": layout.height,
        "nodes": nodes,
        "edges": edges,
    }))
    .map_err(|e| e.to_string())
}

fn child_array(node: &Value) -> Result<&Vec<Value>, String> {
    node.get("children")
        .and_then(Value::as_array)
        .ok_or_else(|| "model: node missing children array".into())
}

fn node_id(node: &Value) -> Result<usize, String> {
    node.get("id")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .ok_or_else(|| "model: node missing id".into())
}

/// Size lookup with the same shape of heuristic fallback the CLI path
/// uses (max line width + insets) so an unmeasured node still occupies
/// plausible space.
fn node_size(node: &Value, sizes: &HashMap<String, (f64, f64)>, em: f64) -> Result<(f64, f64), String> {
    let id = node_id(node)?;
    if let Some(&(w, h)) = sizes.get(&id.to_string()) {
        if w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0 {
            return Ok((w, h));
        }
    }
    let lines: Vec<String> = node
        .get("label")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let text_w = lines
        .iter()
        .map(|l| {
            l.chars()
                .map(|c| if c.is_ascii() { 0.55 * em } else { em })
                .sum::<f64>()
        })
        .fold(0.0, f64::max);
    let text_h = lines.len().max(1) as f64 * 1.2 * em;
    Ok((text_w + 1.6 * em, text_h + 0.8 * em))
}

/// Build the layout input for one subtree, pruning children of folded
/// nodes.
fn build_input(
    node: &Value,
    sizes: &HashMap<String, (f64, f64)>,
    folded: &HashSet<usize>,
    em: f64,
) -> Result<TreeLayoutInput, String> {
    let id = node_id(node)?;
    let size = node_size(node, sizes, em)?;
    let children = if folded.contains(&id) {
        Vec::new()
    } else {
        child_array(node)?
            .iter()
            .map(|c| build_input(c, sizes, folded, em))
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(TreeLayoutInput { id, size, children })
}

/// The mindmap central root participates without its children (they
/// become the left / right branch lists).
fn build_input_shallow(
    node: &Value,
    sizes: &HashMap<String, (f64, f64)>,
    em: f64,
) -> Result<TreeLayoutInput, String> {
    Ok(TreeLayoutInput {
        id: node_id(node)?,
        size: node_size(node, sizes, em)?,
        children: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINDMAP_SRC: &str = "@startmindmap\n* Root\n+ R1\n++ R1a\n- L1\n@endmindmap\n";
    const WBS_SRC: &str = "@startwbs\n* Root\n** A\n*** A1\n** B\n@endwbs\n";

    fn sizes_for(model: &str) -> String {
        // Give every node a fixed 60×20 box.
        let v: Value = serde_json::from_str(model).unwrap();
        let mut ids = Vec::new();
        fn walk(n: &Value, ids: &mut Vec<u64>) {
            ids.push(n["id"].as_u64().unwrap());
            for c in n["children"].as_array().unwrap() {
                walk(c, ids);
            }
        }
        walk(&v["root"], &mut ids);
        let map: HashMap<String, [f64; 2]> =
            ids.into_iter().map(|i| (i.to_string(), [60.0, 20.0])).collect();
        serde_json::to_string(&map).unwrap()
    }

    #[test]
    fn model_carries_preorder_ids_and_sides() {
        let model = model_json(MINDMAP_SRC).unwrap();
        let v: Value = serde_json::from_str(&model).unwrap();
        assert_eq!(v["kind"], "mindmap");
        assert_eq!(v["root"]["id"], 0);
        assert_eq!(v["root"]["children"][0]["id"], 1);
        assert_eq!(v["root"]["children"][0]["children"][0]["id"], 2);
        assert_eq!(v["root"]["children"][1]["id"], 3);
        assert_eq!(v["root"]["children"][1]["side"], "left");
    }

    #[test]
    fn wbs_display_list_places_all_nodes() {
        let model = model_json(WBS_SRC).unwrap();
        let dl = display_list_json(&model, &sizes_for(&model), "[]", 10.0).unwrap();
        let v: Value = serde_json::from_str(&dl).unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), 4);
        assert_eq!(v["edges"].as_array().unwrap().len(), 3);
        assert!(v["width"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn folding_prunes_subtree() {
        let model = model_json(WBS_SRC).unwrap();
        // Fold node 1 ("A") → its child A1 (id 2) disappears.
        let dl = display_list_json(&model, &sizes_for(&model), "[1]", 10.0).unwrap();
        let v: Value = serde_json::from_str(&dl).unwrap();
        let ids: Vec<u64> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![0, 1, 3]);
        assert_eq!(v["edges"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn missing_sizes_fall_back_to_heuristic() {
        let model = model_json(WBS_SRC).unwrap();
        let dl = display_list_json(&model, "{}", "[]", 10.0).unwrap();
        let v: Value = serde_json::from_str(&dl).unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn mindmap_partitions_by_side() {
        let model = model_json(MINDMAP_SRC).unwrap();
        let dl = display_list_json(&model, &sizes_for(&model), "[]", 10.0).unwrap();
        let v: Value = serde_json::from_str(&dl).unwrap();
        let node = |id: u64| -> (f64, f64) {
            let n = v["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["id"].as_u64().unwrap() == id)
                .unwrap();
            (n["x"].as_f64().unwrap(), n["y"].as_f64().unwrap())
        };
        let (root_x, _) = node(0);
        assert!(node(1).0 > root_x, "R1 right of root");
        assert!(node(3).0 < root_x, "L1 left of root");
    }

    #[test]
    fn named_colors_resolve_to_typst_hex() {
        assert_eq!(css_color("#red").as_deref(), Some("#FF4136"));
        assert_eq!(css_color("#FF0000").as_deref(), Some("#FF0000"));
        assert_eq!(css_color("#notacolor"), None);
    }

    #[test]
    fn non_tree_source_errors() {
        let err = model_json("@startuml\nA -> B: hi\n@enduml\n").unwrap_err();
        assert!(err.contains("no mindmap or WBS"), "got: {err}");
    }
}
