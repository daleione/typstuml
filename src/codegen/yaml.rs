//! YAML diagram codegen — thin wrapper over the shared `record-graph` emitter.
//!
//! YAML and JSON share the same data shape after deserialization, so the
//! Typst output is identical to JSON's. See [`super::record_graph`] for the
//! tree-flattening, child-spawning, and markup-escaping rules.

use crate::codegen::record_graph::emit_record_graph;
use crate::ir::YamlDiagram;
use crate::runtime::MeasurementSet;

pub fn emit(
    out: &mut String,
    yaml: &YamlDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    emit_record_graph(
        out,
        yaml.title.as_deref(),
        &yaml.root,
        measurements,
        diagram_idx,
    );
}
