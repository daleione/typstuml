//! WebAssembly bindings for TypstUML.
//!
//! Exposes the filesystem-free render pipeline ([`typstuml::render`]) to
//! JavaScript via `wasm-bindgen`. Callers pass PlantUML source text and get
//! back an SVG string (or PNG / PDF bytes, or the intermediate Typst source).
//!
//! There is no filesystem behind a wasm build: `!include` directives won't
//! resolve and user preambles aren't supported. Everything else — the
//! parsers, the Sugiyama layout, the embedded `blockcell` Typst sources, the
//! Typst compiler itself, and Typst's default fonts — is baked into the
//! `.wasm` module, so rendering is fully self-contained and offline.

use wasm_bindgen::prelude::*;

use typstuml::render;
use typstuml::runtime::Format;

/// Forward Rust panics to `console.error` with a readable stack trace.
///
/// Runs automatically when the module is instantiated (`wasm-bindgen(start)`),
/// so callers don't need to invoke it. Safe to call again by hand.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Render PlantUML `source` to an SVG document string.
///
/// This is the primary entry point for web use. Throws a JS `Error` carrying
/// the diagnostic message on a parse or Typst-compile failure.
#[wasm_bindgen(js_name = renderSvg)]
pub fn render_svg(source: &str) -> Result<String, JsError> {
    let rendered = render::render_source(source, Format::Svg).map_err(to_js)?;
    String::from_utf8(rendered.bytes).map_err(|e| JsError::new(&e.to_string()))
}

/// Render PlantUML `source` to PNG bytes at the given `scale`.
///
/// `scale` is pixels-per-typographic-point (1 pt = 1/72 inch), so 2.0 ≈ 144
/// DPI, 4.0 ≈ 288 DPI. Values are clamped to `[0.5, 16.0]` on the Rust side.
/// Multi-diagram inputs only render the first diagram — use SVG or PDF for
/// those. Returned as a `Uint8Array` on the JS side.
#[wasm_bindgen(js_name = renderPng)]
pub fn render_png(source: &str, scale: f32) -> Result<Vec<u8>, JsError> {
    Ok(render::render_source(source, Format::Png { scale })
        .map_err(to_js)?
        .bytes)
}

/// Render PlantUML `source` to a PDF document.
///
/// Returned as a `Uint8Array` on the JS side.
#[wasm_bindgen(js_name = renderPdf)]
pub fn render_pdf(source: &str) -> Result<Vec<u8>, JsError> {
    Ok(render::render_source(source, Format::Pdf)
        .map_err(to_js)?
        .bytes)
}

/// Emit the generated Typst source for `source` without rendering it —
/// handy for debugging what TypstUML's codegen produced.
#[wasm_bindgen(js_name = emitTypst)]
pub fn emit_typst(source: &str) -> Result<String, JsError> {
    render::emit_typst(source).map_err(to_js)
}

/// Append a font file's faces to the shared font cache. Returns the number
/// of faces that were extracted (TTC files can contain several).
///
/// Designed for fetching CJK / emoji fonts from a CDN at runtime instead of
/// baking them into the .wasm — see the playground's "Fonts" UI. Once added,
/// Typst's automatic fallback picks them up whenever the embedded defaults
/// don't cover a requested glyph; the JS side should kick off a re-render.
///
/// Accepts raw TTF / OTF / TTC bytes. WOFF / WOFF2 are not supported
/// (Typst expects raw font tables; embed a decoder on the JS side first).
///
/// Cfg-gated to wasm32 because the underlying `typstuml::runtime::add_font`
/// is wasm-only — native builds use `typst-kit`'s font searcher and don't
/// need a runtime injection path. The crate is still buildable on the host
/// (via the `rlib` crate-type) for unit tests; this export just isn't
/// available there.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = addFont)]
pub fn add_font(data: &[u8]) -> Result<usize, JsError> {
    typstuml::runtime::add_font(data.to_vec()).map_err(|e| JsError::new(&e))
}

/// Parse PlantUML `source` and return the structural tree model (JSON)
/// for its first `@startmindmap` / `@startwbs` diagram — labels, shapes,
/// colors, and stable pre-order node IDs, no geometry.
///
/// This is the one-time half of the interactive tree pipeline: the JS
/// side measures each node's rendered size once (its own font engine is
/// the ground truth), then calls [`tree_layout`] for coordinates. See
/// `docs/mindmap-web-interactive-design.md` §4.
#[wasm_bindgen(js_name = treeModel)]
pub fn tree_model(source: &str) -> Result<String, JsError> {
    typstuml::web::tree::model_json(source).map_err(|e| JsError::new(&e))
}

/// Compute the display list (JSON: canvas size + per-node x/y/w/h +
/// connector polylines) for a tree model produced by [`tree_model`].
///
/// - `model`: the model JSON, unchanged.
/// - `sizes`: `{"<id>": [w, h], …}` measured node boxes, in px.
/// - `folded`: `[id, …]` — children of these nodes are pruned.
/// - `em`: the renderer's font size in px; every gap constant scales
///   from it (the web analogue of the CLI path's `1em` probe).
///
/// Pure arithmetic — no Typst compile — so calling this on every
/// fold/unfold stays comfortably within a frame.
#[wasm_bindgen(js_name = treeLayout)]
pub fn tree_layout(
    model: &str,
    sizes: &str,
    folded: &str,
    em: f64,
) -> Result<String, JsError> {
    typstuml::web::tree::display_list_json(model, sizes, folded, em)
        .map_err(|e| JsError::new(&e))
}

/// Collapse a [`typstuml::diagnostics::Error`] into a JS `Error`. The
/// `Display` impl already produces a human-readable, line-annotated message.
fn to_js(err: typstuml::diagnostics::Error) -> JsError {
    JsError::new(&err.to_string())
}
