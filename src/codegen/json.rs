//! JSON diagram codegen.
//!
//! Walks the parsed `serde_json::Value` and emits a nested
//! `tree(node[root], child, …)` call. Scalars become single-node leaves
//! labeled `key: value`; nested objects / arrays become subtrees with their
//! own root node (`key: {}` / `key: []`) and children for each entry.
//!
//! Strings are wrapped in Typst markup escaping so backticks, `*`, and other
//! markup characters in JSON values render as literal text rather than being
//! interpreted by Typst.

use crate::ir::JsonDiagram;

pub fn emit(out: &mut String, json: &JsonDiagram) {
    if let Some(title) = &json.title {
        out.push_str("#align(center)[*");
        out.push_str(&typst_markup_escape(title));
        out.push_str("*]\n\n");
    }
    out.push_str("#tree(");
    emit_value(out, &json.root, None);
    out.push_str(")\n");
}

fn emit_value(out: &mut String, value: &serde_json::Value, key: Option<&str>) {
    match value {
        serde_json::Value::Object(map) => {
            emit_node(out, key, "{}");
            for (k, v) in map {
                out.push_str(", ");
                if value_is_compound(v) {
                    out.push_str("tree(");
                    emit_value(out, v, Some(k));
                    out.push(')');
                } else {
                    let label = format_scalar_entry(k, v);
                    emit_node(out, None, &label);
                }
            }
        }
        serde_json::Value::Array(items) => {
            emit_node(out, key, "[]");
            for (i, v) in items.iter().enumerate() {
                let idx = i.to_string();
                out.push_str(", ");
                if value_is_compound(v) {
                    out.push_str("tree(");
                    emit_value(out, v, Some(&idx));
                    out.push(')');
                } else {
                    let label = format_scalar_entry(&idx, v);
                    emit_node(out, None, &label);
                }
            }
        }
        scalar => {
            let body = scalar_to_string(scalar);
            let label = match key {
                Some(k) => format!("{k}: {body}"),
                None => body,
            };
            emit_node(out, None, &label);
        }
    }
}

fn emit_node(out: &mut String, key: Option<&str>, suffix: &str) {
    let label = match key {
        Some(k) => format!("{k}: {suffix}"),
        None => suffix.to_string(),
    };
    out.push_str("node[");
    out.push_str(&typst_markup_escape(&label));
    out.push(']');
}

fn value_is_compound(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(map) => !map.is_empty(),
        serde_json::Value::Array(arr) => !arr.is_empty(),
        _ => false,
    }
}

fn format_scalar_entry(key: &str, value: &serde_json::Value) -> String {
    format!("{key}: {}", scalar_to_string(value))
}

fn scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{s}\""),
        // Compound values land here only when empty — render them inline so
        // we don't synthesize a single-node `tree(...)` for nothing.
        serde_json::Value::Object(_) => "{}".to_string(),
        serde_json::Value::Array(_) => "[]".to_string(),
    }
}

/// Escape characters that have special meaning in Typst markup so JSON values
/// render as literal text. Run before placing the string inside `node[...]`.
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

    fn render(value: serde_json::Value) -> String {
        let mut out = String::new();
        emit(
            &mut out,
            &JsonDiagram {
                name: None,
                title: None,
                root: value,
            },
        );
        out
    }

    #[test]
    fn primitives_emit_single_node() {
        let out = render(json!("hello"));
        assert!(out.contains("node[\"hello\"]"));
    }

    #[test]
    fn object_emits_root_brace_and_scalar_entries() {
        let out = render(json!({"name": "Alice", "age": 30}));
        assert!(out.contains("node[\\{\\}]"));
        assert!(out.contains("node[name: \"Alice\"]"));
        assert!(out.contains("node[age: 30]"));
    }

    #[test]
    fn nested_object_becomes_subtree() {
        let out = render(json!({"addr": {"city": "NYC"}}));
        assert!(out.contains("tree(node[addr: \\{\\}], node[city: \"NYC\"])"));
    }

    #[test]
    fn array_uses_indices() {
        let out = render(json!([true, false, null]));
        assert!(out.contains("node[\\[\\]]"));
        assert!(out.contains("node[0: true]"));
        assert!(out.contains("node[2: null]"));
    }

    #[test]
    fn markup_specials_in_strings_are_escaped() {
        let out = render(json!({"title": "*bold* and #hash"}));
        // The asterisks / hash that come from the JSON value must be escaped.
        assert!(out.contains("title: \"\\*bold\\* and \\#hash\""));
    }
}
