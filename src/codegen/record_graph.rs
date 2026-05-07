//! Shared `#record-layout(...)` emitter used by JSON and YAML codegen.
//!
//! Both diagram types boil down to the same data shape: an arbitrary tree of
//! objects / arrays / scalars rooted at one `serde_json::Value`. The root is
//! flattened into key-value rows and compound values become referenced child
//! records connected by dashed arrows in the rendered diagram.
//!
//! Array transparency: a row whose value is an array does not produce its
//! own intermediate record. When all elements are objects each becomes a
//! direct child record (matching PlantUML's `phoneNumbers` rendering); for
//! scalar / nested-array / mixed arrays the elements are packed into a
//! single child record with index-keyed rows.
//!
//! Layout strategy: we estimate each record's bounding box in Typst pt, run
//! the Sugiyama placer over the resulting graph, then emit absolute record
//! positions plus per-edge cubic bezier control points. Typst-side
//! `record-layout` is a thin painter — it re-measures and snaps endpoints
//! to its own rendered geometry, so our pt estimate need only be close
//! enough that the placer doesn't overlap records.
//!
//! Both Rust- and Typst-side measurements assume a 10pt sans-serif body
//! and `inset: (x: 0.5em, y: 0.25em)`; if the Typst defaults change, the
//! constants below need to track them.

use serde_json::Value;

use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};

/// Document body font size — set by the prelude `#set text(size: 10pt)`
/// the codegen emits. All other measurements are derived from it.
const FONT_PT: f64 = 10.0;
/// Bold key glyphs run wider than the body. 0.60em is conservative for
/// 10pt sans-serif (Typst defaults, Linux Libertine / DejaVu).
const KEY_EM: f64 = 0.60;
/// Body glyph width. 0.55em is conservative for ASCII; CJK and emoji are
/// detected separately and treated as 1em.
const VAL_EM: f64 = 0.55;
/// Cell padding, matching Typst-side `inset`.
const PAD_X_PT: f64 = 0.5 * FONT_PT;
const PAD_Y_PT: f64 = 0.25 * FONT_PT;
/// Default Typst leading at 10pt.
const LINE_HEIGHT_PT: f64 = 1.2 * FONT_PT;
/// Origin-dot radius (matches `record-layout`'s `arrow-dot` default of
/// 3pt). The value column is forced to fit `4 * arrow-dot` so an
/// all-compound record still leaves room for the dot.
const ARROW_DOT_PT: f64 = 3.0;
const VALUE_MIN_PT: f64 = 4.0 * ARROW_DOT_PT;
/// Maximum bezier control-handle pull at each anchor. The actual pull is
/// the smaller of this and ~half the horizontal distance between source
/// and target — short hops should not over-sweep, long hops should still
/// flare enough to leave the box edge cleanly.
const EDGE_FORCE_MAX_PT: f64 = 30.0;

pub fn emit_record_graph(out: &mut String, title: Option<&str>, root: &Value) {
    let specs = flatten(root);
    let geoms: Vec<RecordGeom> = specs.iter().map(record_geom).collect();

    let mut vg = VisualGraph::new(Orientation::LeftToRight);
    let handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node(Element::new_box(g.size, Orientation::LeftToRight)))
        .collect();

    for (parent_idx, spec) in specs.iter().enumerate() {
        for (row_idx, child_idx) in &spec.children {
            vg.add_edge(
                Edge { src_row: *row_idx },
                handles[parent_idx],
                handles[*child_idx],
            );
        }
    }

    vg.layout();

    let top_lefts: Vec<Point> = handles.iter().map(|h| vg.pos(*h).bbox(false).0).collect();

    out.push_str("#record-layout(\n");
    if let Some(title) = title {
        out.push_str("  title: [");
        out.push_str(&typst_markup_escape(title));
        out.push_str("],\n");
    }

    out.push_str("  records: (\n");
    for (i, spec) in specs.iter().enumerate() {
        emit_record(out, top_lefts[i], &spec.rows);
    }
    out.push_str("  ),\n");

    out.push_str("  edges: (\n");
    for (edge, path) in vg.iter_edges() {
        let from_idx = path[0].get_index();
        let to_idx = path[path.len() - 1].get_index();
        let start = source_anchor(&geoms[from_idx], top_lefts[from_idx], edge.src_row);
        let end = target_anchor(&geoms[to_idx], top_lefts[to_idx]);
        let (c1, c2) = bezier_controls(start, end);
        emit_edge(out, from_idx, edge.src_row, to_idx, c1, c2);
    }
    out.push_str("  ),\n");

    out.push_str(")\n");
}

/// One record we want shown — flattened from a JSON node.
struct RecordSpec {
    rows: Vec<Row>,
    /// Outgoing references: `(parent row index, target spec index)`.
    children: Vec<(usize, usize)>,
}

struct Row {
    key: Option<String>,
    /// Already escaped for Typst markup. Empty for compound values — Typst
    /// `record-layout` paints the origin dot inside an empty cell.
    value: String,
}

fn flatten(root: &Value) -> Vec<RecordSpec> {
    let mut specs = Vec::new();
    build_spec(&mut specs, root);
    specs
}

fn build_spec(specs: &mut Vec<RecordSpec>, value: &Value) -> usize {
    let my_idx = specs.len();
    specs.push(RecordSpec {
        rows: Vec::new(),
        children: Vec::new(),
    });

    let entries = node_entries(value);
    let mut rows = Vec::with_capacity(entries.len());
    let mut children = Vec::new();
    for (row_idx, entry) in entries.iter().enumerate() {
        rows.push(Row {
            key: entry.key.clone(),
            value: render_value(&entry.value),
        });
        for child_value in spawn_children(&entry.value) {
            let child_idx = build_spec(specs, &child_value);
            children.push((row_idx, child_idx));
        }
    }
    specs[my_idx].rows = rows;
    specs[my_idx].children = children;
    my_idx
}

struct NodeEntry {
    key: Option<String>,
    value: Value,
}

fn node_entries(value: &Value) -> Vec<NodeEntry> {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| NodeEntry {
                key: Some(k.clone()),
                value: v.clone(),
            })
            .collect(),
        Value::Array(arr) => arr
            .iter()
            .enumerate()
            .map(|(i, v)| NodeEntry {
                key: Some(i.to_string()),
                value: v.clone(),
            })
            .collect(),
        scalar => vec![NodeEntry {
            key: None,
            value: scalar.clone(),
        }],
    }
}

/// PlantUML semantics: every non-empty compound value becomes exactly one
/// child record. Arrays of objects therefore go through an intermediate
/// index-keyed array record, matching `phoneNumbers` rendering.
fn spawn_children(value: &Value) -> Vec<Value> {
    match value {
        Value::Object(m) if !m.is_empty() => vec![value.clone()],
        Value::Array(a) if !a.is_empty() => vec![value.clone()],
        _ => vec![],
    }
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "␀".to_string(),
        Value::Bool(true) => "☑ true".to_string(),
        Value::Bool(false) => "☒ false".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{}\"", typst_markup_escape(s)),
        Value::Object(_) | Value::Array(_) => String::new(),
    }
}

/// Width contribution of one glyph at body font size. CJK / emoji / other
/// non-ASCII default to a full em — true 2-column-wide CJK is closer to
/// 1em in Typst's default fonts and an over-estimate is harmless for layout.
fn glyph_width_pt(c: char, em: f64) -> f64 {
    if c.is_ascii() {
        FONT_PT * em
    } else {
        FONT_PT
    }
}

fn text_width_pt(s: &str, em: f64) -> f64 {
    s.chars().map(|c| glyph_width_pt(c, em)).sum()
}

/// Per-record geometry the codegen needs at emit time: total bounding box
/// (passed to the placer) and per-row vertical centers (used to anchor
/// edge endpoints to the same row the painter does — see
/// `record-layout`'s `resolve-start`). Both are measured in Typst pt and
/// derived from the same constants the painter uses, so codegen and
/// painter agree on row positions.
struct RecordGeom {
    size: Point,
    row_centers: Vec<f64>,
}

fn record_geom(spec: &RecordSpec) -> RecordGeom {
    if spec.rows.is_empty() {
        return RecordGeom { size: Point::new(1., 1.), row_centers: Vec::new() };
    }
    let key_w = spec
        .rows
        .iter()
        .map(|r| {
            r.key
                .as_deref()
                .map(|k| text_width_pt(k, KEY_EM))
                .unwrap_or(0.)
        })
        .fold(0., f64::max)
        + 2. * PAD_X_PT;
    let val_w = spec
        .rows
        .iter()
        .map(|r| text_width_pt(&r.value, VAL_EM))
        .fold(0., f64::max)
        .max(VALUE_MIN_PT)
        + 2. * PAD_X_PT;
    let row_h = LINE_HEIGHT_PT + 2. * PAD_Y_PT;
    let row_centers = (0..spec.rows.len())
        .map(|i| (i as f64 + 0.5) * row_h)
        .collect();
    RecordGeom {
        size: Point::new(key_w + val_w, row_h * spec.rows.len() as f64),
        row_centers,
    }
}

/// Edge start anchor: just inside the right edge of the source row's value
/// cell, at that row's vertical center — matches `record-layout`'s
/// `resolve-start` so codegen-emitted control points line up with the
/// painter's snapped endpoint.
fn source_anchor(g: &RecordGeom, top_left: Point, src_row: usize) -> Point {
    let y = top_left.y + g.row_centers[src_row];
    let x = top_left.x + g.size.x - PAD_X_PT - ARROW_DOT_PT;
    Point::new(x, y)
}

/// Edge end anchor: left edge of the target record at its vertical center.
fn target_anchor(g: &RecordGeom, top_left: Point) -> Point {
    Point::new(top_left.x, top_left.y + g.size.y / 2.)
}

/// Bezier control points for a single cubic from `start` to `end`. The
/// graph is laid out left-to-right, so we pull horizontally — control
/// handles always exit / enter parallel to the box edges. The pull is
/// capped at `EDGE_FORCE_MAX_PT` and at half the horizontal gap, so short
/// edges don't overshoot into a hump and long edges still leave their
/// boxes cleanly.
fn bezier_controls(start: Point, end: Point) -> (Point, Point) {
    let force = EDGE_FORCE_MAX_PT.min((end.x - start.x).abs() * 0.5);
    (
        Point::new(start.x + force, start.y),
        Point::new(end.x - force, end.y),
    )
}

fn emit_record(out: &mut String, top_left: Point, rows: &[Row]) {
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, rows: (",
        top_left.x, top_left.y,
    ));
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str("(key: ");
        match &row.key {
            Some(k) => {
                out.push('[');
                out.push_str(&typst_markup_escape(k));
                out.push(']');
            }
            None => out.push_str("none"),
        }
        out.push_str(", value: [");
        out.push_str(&row.value);
        out.push_str("])");
    }
    if rows.len() == 1 {
        // Trailing comma — single-element tuple disambiguation.
        out.push(',');
    }
    out.push_str(")),\n");
}

/// Emit one edge with `from` / `from-row` / `to` indices plus the bezier
/// control points. The painter re-snaps endpoints to its own measured
/// geometry; we deliberately compute `c1` / `c2` against the same
/// row anchors, so curve and snapped endpoints stay aligned.
fn emit_edge(
    out: &mut String,
    from_idx: usize,
    from_row: usize,
    to_idx: usize,
    c1: Point,
    c2: Point,
) {
    out.push_str(&format!(
        "    (from: {from_idx}, from-row: {from_row}, to: {to_idx}, c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt)),\n",
        c1.x, c1.y, c2.x, c2.y,
    ));
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
    use serde_json::json;

    fn render(title: Option<&str>, value: serde_json::Value) -> String {
        let mut out = String::new();
        emit_record_graph(&mut out, title, &value);
        out
    }

    #[test]
    fn root_emits_record_layout_call() {
        let out = render(None, json!({"name": "Alice"}));
        assert!(out.starts_with("#record-layout(\n"));
        assert!(out.contains("(key: [name], value: [\"Alice\"])"));
    }

    #[test]
    fn nested_object_becomes_separate_record() {
        let out = render(None, json!({"addr": {"city": "NYC"}}));
        assert!(out.contains("records: ("));
        let record_lines = out.matches("rows: (").count();
        assert_eq!(record_lines, 2);
        assert!(out.contains("(key: [addr], value: [])"));
        assert!(out.contains("(key: [city], value: [\"NYC\"])"));
    }

    #[test]
    fn object_array_inserts_intermediate_index_record() {
        // PlantUML semantics: the array becomes its own record with
        // index-keyed rows (0, 1), each pointing to its element.
        let out = render(None, json!({"phones": [{"n": 1}, {"n": 2}]}));
        assert_eq!(out.matches("rows: (").count(), 4);
        assert_eq!(out.matches("(from: ").count(), 3);
        assert!(out.contains("(key: [0], value: [])"));
        assert!(out.contains("(key: [1], value: [])"));
    }

    #[test]
    fn empty_compound_emits_no_child() {
        let out = render(None, json!({"empty-arr": [], "empty-obj": {}}));
        assert_eq!(out.matches("rows: (").count(), 1);
        assert_eq!(out.matches("(from: ").count(), 0);
        assert!(out.contains("(key: [empty-arr], value: [])"));
        assert!(out.contains("(key: [empty-obj], value: [])"));
    }

    #[test]
    fn scalars_get_special_markers() {
        let out = render(None, json!({"a": true, "b": false, "c": null}));
        assert!(out.contains("value: [☑ true]"));
        assert!(out.contains("value: [☒ false]"));
        assert!(out.contains("value: [␀]"));
    }

    #[test]
    fn markup_specials_in_strings_are_escaped() {
        let out = render(None, json!({"title": "*bold* and #hash"}));
        assert!(out.contains("\"\\*bold\\* and \\#hash\""));
    }

    #[test]
    fn title_is_emitted_in_record_layout_param() {
        let out = render(Some("My data"), json!({"x": 1}));
        assert!(out.contains("title: [My data]"));
    }

    #[test]
    fn root_array_packs_into_single_record() {
        let out = render(None, json!([1, 2, 3]));
        assert_eq!(out.matches("rows: (").count(), 1);
        assert!(out.contains("(key: [0], value: [1])"));
        assert!(out.contains("(key: [2], value: [3])"));
    }

    #[test]
    fn record_positions_are_emitted() {
        let out = render(None, json!({"a": {"b": 1}}));
        let mut rest = out.as_str();
        let mut count = 0;
        while let Some(i) = rest.find("x: ") {
            let after = &rest[i + 3..];
            if let Some(j) = after.find("pt, y:") {
                count += 1;
                rest = &after[j..];
            } else {
                break;
            }
        }
        assert!(count >= 2, "expected per-record (x, y) tuples; got: {out}");
    }

    #[test]
    fn edges_carry_from_to_indices() {
        let out = render(None, json!({"a": {"b": 1}}));
        assert!(out.contains("from: 0, from-row: 0, to: 1"));
    }
}
