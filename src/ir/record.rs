//! JSON / YAML record-graph diagrams. Both parse into a `serde_json::Value`
//! tree and share the record-graph codegen path.

#[derive(Clone, Debug)]
pub struct JsonDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    /// Parsed JSON value. The full serde_json::Value tree is the AST — there's
    /// no further normalization since `tree` codegen walks it recursively.
    pub root: serde_json::Value,
}

/// YAML diagram. Parsed via `serde_yaml_ng` directly into a
/// `serde_json::Value` so it can share the JSON record-graph codegen path —
/// the rendered output for an equivalent JSON / YAML document is identical.
#[derive(Clone, Debug)]
pub struct YamlDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub root: serde_json::Value,
}
