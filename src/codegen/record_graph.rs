//! Shared `#record-graph(...)` emitter used by JSON and YAML codegen.
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
//! Strings are escaped for Typst content markup so backticks, `*`, `#`,
//! etc. in source values render literally instead of being interpreted.
//!
//! The compound-value placeholder (`~~~`) gives empty value cells visible
//! width so the column doesn't collapse on all-compound objects; on
//! object-with-scalar rows the wider scalar values dominate the column
//! and the placeholder is invisible.

use serde_json::Value;

pub fn emit_record_graph(out: &mut String, title: Option<&str>, root: &Value) {
    out.push_str("#record-graph(");
    if let Some(title) = title {
        out.push_str("title: [");
        out.push_str(&typst_markup_escape(title));
        out.push_str("], ");
    }
    emit_node(out, root);
    out.push_str(")\n");
}

/// Emit one node — a `(rows: (...), children: (...))` dict — for `value`.
fn emit_node(out: &mut String, value: &Value) {
    let entries = node_entries(value);

    out.push_str("(rows: (");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_row(out, e.key.as_deref(), &e.value);
    }
    if entries.len() == 1 {
        // Trailing comma so a single-entry tuple parses as an array, not a
        // parenthesized expression.
        out.push(',');
    }
    out.push_str("), children: (");

    let mut first_child = true;
    for (i, e) in entries.iter().enumerate() {
        for child in spawn_children(&e.value) {
            if first_child {
                first_child = false;
            } else {
                out.push_str(", ");
            }
            out.push_str(&format!("(row: {i}, node: "));
            emit_node(out, &child);
            out.push(')');
        }
    }
    out.push_str("))");
}

struct NodeEntry {
    key: Option<String>,
    value: Value,
}

/// Flatten one value into the rows that will populate its record.
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

/// Determine which sub-records a row's value spawns. Object → one child;
/// non-empty object-only array → one child per element (transparent array);
/// non-empty other array → one packed child; scalar / empty → none.
fn spawn_children(value: &Value) -> Vec<Value> {
    match value {
        Value::Object(map) if !map.is_empty() => vec![value.clone()],
        Value::Array(arr) if !arr.is_empty() => {
            if arr.iter().all(|v| matches!(v, Value::Object(_))) {
                arr.clone()
            } else {
                vec![value.clone()]
            }
        }
        _ => vec![],
    }
}

fn emit_row(out: &mut String, key: Option<&str>, value: &Value) {
    out.push_str("(key: ");
    match key {
        Some(k) => {
            out.push('[');
            out.push_str(&typst_markup_escape(k));
            out.push(']');
        }
        None => out.push_str("none"),
    }
    out.push_str(", value: [");
    out.push_str(&render_value(value));
    out.push_str("])");
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "␀".to_string(),
        Value::Bool(true) => "☑ true".to_string(),
        Value::Bool(false) => "☒ false".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{}\"", typst_markup_escape(s)),
        // Compound — empty cell with a width-preserving placeholder so the
        // value column stays visible even when every row points elsewhere.
        // `~` is Typst markup for non-breaking space.
        Value::Object(_) | Value::Array(_) => "~~~".to_string(),
    }
}

/// Escape characters that have special meaning in Typst markup so source
/// values render as literal text. Run before placing the string inside
/// `[...]` content.
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
    fn root_object_emits_record_graph_with_rows() {
        let out = render(None, json!({"name": "Alice", "age": 30}));
        assert!(out.starts_with("#record-graph("));
        assert!(out.contains("(key: [name], value: [\"Alice\"])"));
        assert!(out.contains("(key: [age], value: [30])"));
    }

    #[test]
    fn nested_object_becomes_child_record() {
        let out = render(None, json!({"addr": {"city": "NYC"}}));
        assert!(out.contains("(key: [addr], value: [~~~])"));
        assert!(out.contains("(row: 0, node: (rows: ((key: [city], value: [\"NYC\"]),)"));
    }

    #[test]
    fn object_array_spawns_one_child_per_element() {
        let out = render(None, json!({"phones": [{"n": 1}, {"n": 2}]}));
        assert_eq!(out.matches("(row: 0, node:").count(), 2);
        assert!(out.contains("(key: [n], value: [1])"));
        assert!(out.contains("(key: [n], value: [2])"));
    }

    #[test]
    fn empty_compound_emits_no_child() {
        let out = render(None, json!({"empty-arr": [], "empty-obj": {}}));
        assert!(out.contains("(key: [empty-arr], value: [~~~])"));
        assert!(out.contains("(key: [empty-obj], value: [~~~])"));
        assert!(!out.contains("(row:"));
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
    fn root_array_packs_into_single_record() {
        let out = render(None, json!([1, 2, 3]));
        assert!(out.contains("(key: [0], value: [1])"));
        assert!(out.contains("(key: [2], value: [3])"));
        assert!(!out.contains("(row:"));
    }

    #[test]
    fn root_scalar_is_keyless_row() {
        let out = render(None, json!("hello"));
        assert!(out.contains("(key: none, value: [\"hello\"]),"));
    }

    #[test]
    fn title_is_emitted_in_record_graph_param() {
        let out = render(Some("My data"), json!({"x": 1}));
        assert!(out.starts_with("#record-graph(title: [My data], "));
    }
}
