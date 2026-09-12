//! Legacy PlantUML ABI; retains permissive measurement decoding.
use super::{
    __ToResult, __send_result_to_host, __write_args_to_buffer,
    wire::{self, Validation},
};
use wasm_minimal_protocol::wasm_func;

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

    let set = wire::decode(measurements_cbor, Validation::Legacy)?;

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
