//! Typst host ABI. Language-neutral exports delegate to isolated protocol versions.
use wasm_minimal_protocol::*;
initiate_protocol!();
mod v1;
mod v2;
mod wire;
pub use v1::{emit_layout, emit_layout_no_measure, emit_probes};
pub use v2::{emit_layout_no_measure_v2, emit_layout_v2, emit_probes_v2};
const PROTOCOL_VERSION: u32 = 2;

#[wasm_func]
pub fn protocol_version() -> Result<Vec<u8>, String> {
    Ok(PROTOCOL_VERSION.to_le_bytes().to_vec())
}
#[wasm_func]
pub fn referenced_symbols() -> Result<Vec<u8>, String> {
    serde_json::to_vec(typstuml::codegen::REFERENCED_BLOCKCELL_SYMBOLS).map_err(|e| e.to_string())
}
