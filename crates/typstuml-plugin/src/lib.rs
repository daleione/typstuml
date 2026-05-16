//! Typst plugin entry — exposes TypstUML's PlantUML→Typst codegen to a
//! Typst document via the `#plugin(...)` host ABI.
//!
//! ## Two-phase contract
//!
//! 1. `emit_probes(source) -> bytes`
//!    Returns the pass-1 (probe-only) Typst source the Typst-side
//!    `lib.typ` should `hide(eval(...))` to populate introspection.
//!    Empty payload = "no measurement-aware diagrams; skip pass-1 and
//!    go straight to `emit_layout_no_measure`".
//!
//! 2. `emit_layout(source, measurements_cbor) -> bytes`
//!    Returns the pass-2 Typst source. `measurements_cbor` is the CBOR
//!    of an `Array<{id, width_pt, height_pt, row_centers?}>` collected
//!    from `query(<typstuml_measure>)`.
//!
//! 3. `emit_layout_no_measure(source) -> bytes`
//!    Fast path when pass-1 was skipped.
//!
//! 4. `referenced_symbols() -> bytes`
//!    JSON array of blockcell symbol names this version of codegen
//!    emits. `lib.typ` uses it to assert its `eval` scope still covers
//!    everything — a drift guard for when codegen grows new symbols.
//!
//! 5. `protocol_version() -> bytes`
//!    Little-endian `u32` (4 bytes). Bumped on any wire-format break.
//!    `lib.typ` calls this once at load time and panics on mismatch.
//!
//! Errors come back as `Err(String)` which Typst surfaces at the plugin
//! call site.

use serde::{Deserialize, Serialize};
use wasm_minimal_protocol::*;

initiate_protocol!();

/// Bump when the wire format changes incompatibly. `lib.typ` pins the
/// matching value.
const PROTOCOL_VERSION: u32 = 1;

/// Wire shape for one measurement: codegen-assigned ID + dimensions.
/// `row_centers` is optional (only record-graph probes write it); other
/// probes emit an empty array, which CBOR encodes as zero bytes via the
/// `skip_serializing_if` below — keeps payloads small.
#[derive(Serialize, Deserialize)]
struct ProbeEntry {
    id: String,
    width_pt: f64,
    height_pt: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    row_centers: Vec<f64>,
}

#[wasm_func]
pub fn protocol_version() -> Result<Vec<u8>, String> {
    Ok(PROTOCOL_VERSION.to_le_bytes().to_vec())
}

#[wasm_func]
pub fn referenced_symbols() -> Result<Vec<u8>, String> {
    serde_json::to_vec(typstuml::codegen::REFERENCED_BLOCKCELL_SYMBOLS)
        .map_err(|e| e.to_string())
}

#[wasm_func]
pub fn emit_probes(source: &[u8]) -> Result<Vec<u8>, String> {
    let src = std::str::from_utf8(source).map_err(|e| e.to_string())?;
    match typstuml::render::emit_probes_for_plugin(src).map_err(|e| e.to_string())? {
        Some(s) => Ok(s.into_bytes()),
        None => Ok(Vec::new()),
    }
}

#[wasm_func]
pub fn emit_layout(source: &[u8], measurements_cbor: &[u8]) -> Result<Vec<u8>, String> {
    let src = std::str::from_utf8(source).map_err(|e| e.to_string())?;

    let probes: Vec<ProbeEntry> = ciborium::from_reader(measurements_cbor)
        .map_err(|e| format!("decode measurements: {e}"))?;

    let mut set = typstuml::runtime::MeasurementSet::default();
    for p in probes {
        let mut m = typstuml::runtime::Measurement::new(p.width_pt, p.height_pt);
        m.row_centers = p.row_centers;
        set.insert(p.id, m);
    }

    Ok(typstuml::render::emit_layout_for_plugin(src, &set)
        .map_err(|e| e.to_string())?
        .into_bytes())
}

#[wasm_func]
pub fn emit_layout_no_measure(source: &[u8]) -> Result<Vec<u8>, String> {
    let src = std::str::from_utf8(source).map_err(|e| e.to_string())?;
    Ok(typstuml::render::emit_layout_no_measure(src)
        .map_err(|e| e.to_string())?
        .into_bytes())
}
