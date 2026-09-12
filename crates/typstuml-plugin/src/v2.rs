//! Explicit-language ABI with validated measurement bundles.
use super::{
    __ToResult, __send_result_to_host, __write_args_to_buffer,
    wire::{self, Validation},
};
use wasm_minimal_protocol::wasm_func;

fn language(bytes: &[u8]) -> Result<typstuml::parser::InputLanguage, String> {
    std::str::from_utf8(bytes)
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e: typstuml::diagnostics::Error| e.to_string())
}
#[wasm_func]
pub fn emit_probes_v2(lang: &[u8], source: &[u8]) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(source).map_err(|e| e.to_string())?;
    let bundle =
        typstuml::render::emit_probe_bundle(source, language(lang)?).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    ciborium::into_writer(&bundle, &mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}
#[wasm_func]
pub fn emit_layout_v2(
    lang: &[u8],
    source: &[u8],
    measurements_cbor: &[u8],
) -> Result<Vec<u8>, String> {
    let lang = language(lang)?;
    let source = std::str::from_utf8(source).map_err(|e| e.to_string())?;
    let set = wire::decode(measurements_cbor, Validation::Strict)?;
    typstuml::render::emit_layout_with_language(source, lang, Some(&set))
        .map(String::into_bytes)
        .map_err(|e| e.to_string())
}
#[wasm_func]
pub fn emit_layout_no_measure_v2(lang: &[u8], source: &[u8]) -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(source).map_err(|e| e.to_string())?;
    typstuml::render::emit_layout_with_language(source, language(lang)?, None)
        .map(String::into_bytes)
        .map_err(|e| e.to_string())
}
