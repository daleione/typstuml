//! JSON diagram codegen — thin wrapper over the shared `record-graph` emitter.
//!
//! The actual tree-flattening and Typst markup escaping live in
//! [`super::record_graph`] so YAML codegen can reuse the same output shape.

use crate::codegen::record_graph::emit_record_graph;
use crate::ir::JsonDiagram;
use crate::runtime::MeasurementSet;

pub fn emit(
    out: &mut String,
    json: &JsonDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    emit_record_graph(
        out,
        json.title.as_deref(),
        &json.root,
        measurements,
        diagram_idx,
    );
}
