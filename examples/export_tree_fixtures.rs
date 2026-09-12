//! Export deterministic tree-layout fixtures for cross-language parity tests.
//!
//! For every fixture in `tests/fixtures/{mindmap,wbs}/*.puml` this
//! renders the web model JSON plus, for a battery of fold states, the
//! Rust engine's display list — the ground truth the TS layout must
//! reproduce to <1e-6.
//!
//! Usage: cargo run --example export_tree_fixtures -- <output-dir>
//!
//! Sizes are generated with the same heuristic `web/tree.rs::node_size`
//! falls back to, but passed explicitly so both engines lay out from
//! identical inputs. Deterministic output — rerun after editing
//! `src/layout/tree.rs` and re-run the consuming implementation's parity tests.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

const EM: f64 = 12.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let out_dir = args
        .next()
        .ok_or("usage: export_tree_fixtures <output-dir>")?;
    if args.next().is_some() {
        return Err("usage: export_tree_fixtures <output-dir>".into());
    }
    let out_dir = Path::new(&out_dir);
    std::fs::create_dir_all(out_dir)?;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut count = 0;
    for group in ["mindmap", "wbs"] {
        let dir = root.join("tests/fixtures").join(group);
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "puml"))
            .collect();
        entries.sort();
        for path in entries {
            let stem = path.file_stem().unwrap().to_string_lossy();
            let source = std::fs::read_to_string(&path).unwrap();
            let model_str = match typstuml::web::tree::model_json(&source) {
                Ok(m) => m,
                Err(e) => panic!("{}: {e}", path.display()),
            };
            let model: Value = serde_json::from_str(&model_str).unwrap();

            let sizes = sizes_for(&model);
            let sizes_str = serde_json::to_string(&sizes).unwrap();

            let mut cases = Vec::new();
            let mut folded_sets: Vec<Vec<Value>> = vec![Vec::new()];
            let foldable = foldable_ids(&model);
            for id in &foldable {
                folded_sets.push(vec![json!(id)]);
            }
            if foldable.len() > 1 {
                folded_sets.push(foldable.iter().map(|id| json!(id)).collect());
            }
            if model["kind"] == "mindmap" {
                folded_sets.push(vec![json!("left")]);
                folded_sets.push(vec![json!("right")]);
            }
            for folded in folded_sets {
                let folded_str = serde_json::to_string(&folded).unwrap();
                let dl =
                    typstuml::web::tree::display_list_json(&model_str, &sizes_str, &folded_str, EM)
                        .unwrap_or_else(|e| panic!("{} folded={folded_str}: {e}", path.display()));
                cases.push(json!({
                    "folded": folded,
                    "expected": serde_json::from_str::<Value>(&dl).unwrap(),
                }));
            }

            let fixture = json!({
                "source": format!("tests/fixtures/{group}/{stem}.puml"),
                "em": EM,
                "model": model,
                "sizes": sizes,
                "cases": cases,
            });
            let out_path = out_dir.join(format!("{group}-{stem}.json"));
            std::fs::write(&out_path, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();
            count += 1;
        }
    }
    println!("exported {count} fixtures to {}", out_dir.display());
    Ok(())
}

/// Deterministic per-node sizes: the `web/tree.rs::node_size` heuristic
/// applied to every non-phantom node, keyed by numeric id (BTreeMap for
/// stable output order).
///
/// Rounded to one decimal, and that is load-bearing: serde_json's
/// default f64 parse (no `float_roundtrip` feature) is a fast path that
/// can land 1-2 ulp off the correctly-rounded value, while JS
/// `JSON.parse` rounds correctly — a full-precision size like
/// 98.39999999999999 parses to DIFFERENT f64s in the two engines and
/// the divergence shows up as collinear-merge flips in the polylines.
/// Short decimals (mantissa/10) parse identically on both sides.
fn sizes_for(model: &Value) -> BTreeMap<String, [f64; 2]> {
    let mut out = BTreeMap::new();
    fn walk(node: &Value, out: &mut BTreeMap<String, [f64; 2]>) {
        let id = node["id"].as_u64().unwrap();
        if node["shape"] != "phantom" {
            let lines: Vec<&str> = node["label"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let text_w = lines
                .iter()
                .map(|l| {
                    l.chars()
                        .map(|c| if c.is_ascii() { 0.55 * EM } else { EM })
                        .sum::<f64>()
                })
                .fold(0.0, f64::max);
            let text_h = lines.len().max(1) as f64 * 1.2 * EM;
            let round1 = |v: f64| (v * 10.0).round() / 10.0;
            out.insert(
                id.to_string(),
                [round1(text_w + 1.6 * EM), round1(text_h + 0.8 * EM)],
            );
        }
        for c in node["children"].as_array().unwrap() {
            walk(c, out);
        }
    }
    for root in model["roots"].as_array().unwrap() {
        walk(root, &mut out);
    }
    out
}

/// Pre-order ids of nodes with children (fold candidates), phantoms
/// included — folding a phantom is the zero-size-blob regression case.
fn foldable_ids(model: &Value) -> Vec<u64> {
    let mut out = Vec::new();
    fn walk(node: &Value, out: &mut Vec<u64>) {
        let kids = node["children"].as_array().unwrap();
        if !kids.is_empty() {
            out.push(node["id"].as_u64().unwrap());
        }
        for c in kids {
            walk(c, out);
        }
    }
    for root in model["roots"].as_array().unwrap() {
        walk(root, &mut out);
    }
    out
}
