//! Filesystem-free orchestration of the parse → measure → codegen → render
//! pipeline.
//!
//! The native [`crate::cli`] has its own orchestration that layers `!include`
//! resolution, on-disk user preambles, the measure double-pass with verbose
//! timing, and stderr reporting on top. This module is the slim path that
//! embedders — wasm builds, library users — call into: PlantUML source text
//! in, encoded bytes out, no OS underneath required.
//!
//! It deliberately mirrors a subset of `cli`'s pipeline rather than sharing
//! one orchestrator: the two genuinely differ (a project root and an on-disk
//! preamble vs. neither, a `GlobalCtx` reporting sink vs. silent fallback), so
//! the small duplication of the measure double-pass below buys full isolation
//! of the working CLI from the embed path.

use crate::diagnostics::{CompatMode, Error, Result};
#[cfg(feature = "embed-typst")]
use crate::runtime;
use crate::theme::Theme;

#[cfg(feature = "embed-typst")]
pub use crate::runtime::{Format, RenderSize, Rendered};

/// Parse PlantUML `source` and render every diagram it contains to `format`,
/// returning the encoded bytes plus structured parser, measure, and renderer
/// warnings.
///
/// Pure in-memory: `!include` directives won't resolve (they surface as parse
/// warnings) and no user preamble is applied. For filesystem-aware rendering
/// use the CLI.
#[cfg(feature = "embed-typst")]
pub fn render_source(source: &str, format: Format) -> Result<Rendered> {
    let parsed = parse(source)?;
    let (typst_source, mut warnings) = build_typst_source(&parsed.document)?;
    warnings.splice(0..0, parsed.diagnostics);
    let mut rendered = runtime::render(typst_source, None, format)?;
    warnings.append(&mut rendered.warnings);
    rendered.warnings = warnings;
    Ok(rendered)
}

/// Emit the generated Typst source for `source` without rendering it — the
/// in-memory equivalent of the `emit` subcommand. Useful for debugging
/// codegen output from an embedder.
#[cfg(feature = "embed-typst")]
pub fn emit_typst(source: &str) -> Result<String> {
    let parsed = parse(source)?;
    build_typst_source(&parsed.document).map(|(source, _warnings)| source)
}

/// Parse `source` in `Warn` compat mode with no include paths or source dir.
fn parse(source: &str) -> Result<crate::parser::ParseOutput> {
    let config = crate::parser::Config::default();
    let parsed = crate::parser::parse(source, CompatMode::Warn, &config)?;
    if parsed.document.diagrams.is_empty() {
        return Err(Error::Cli("no supported diagrams found in input".into()));
    }
    Ok(parsed)
}

/// Build the pass-2 Typst source for `doc`, running the measure double-pass
/// when the document has measurement-aware diagrams.
///
/// Unlike `cli::build_typst_source` this uses the default (empty) theme and no
/// project root — there is no on-disk preamble to inject and local
/// `#image()`/`#read()` paths are denied. A measure-pass failure is non-fatal:
/// codegen falls back to the Rust-side heuristic and returns a warning.
#[cfg(feature = "embed-typst")]
fn build_typst_source(
    doc: &crate::ir::Document,
) -> Result<(String, Vec<crate::diagnostics::Diagnostic>)> {
    use crate::codegen::ImportStrategy;
    use crate::diagnostics::Diagnostic;
    let theme = Theme::default();

    let Some((probe_source, expected_ids)) =
        crate::codegen::emit_probes(doc, &theme, ImportStrategy::VirtualFs)?
    else {
        // No measurement-aware diagrams — skip pass-1 entirely.
        return crate::codegen::emit(doc, &theme, None, ImportStrategy::VirtualFs)
            .map(|source| (source, Vec::new()));
    };

    let expected_refs: Vec<&str> = expected_ids.iter().map(String::as_str).collect();
    match runtime::measure::run(probe_source, None, &expected_refs) {
        Ok(set) => crate::codegen::emit(doc, &theme, Some(&set), ImportStrategy::VirtualFs)
            .map(|source| (source, Vec::new())),
        Err(error) => {
            crate::codegen::emit(doc, &theme, None, ImportStrategy::VirtualFs).map(|source| {
                (
                    source,
                    vec![Diagnostic::warning(format!(
                        "measure pass failed ({error}); falling back to heuristic"
                    ))],
                )
            })
        }
    }
}

// =========================================================================
// Typst-plugin entry points (used by `crates/typstuml-plugin`).
//
// These mirror `build_typst_source` but split it into the two halves
// `crate::codegen::emit_probes` and `crate::codegen::emit` so the
// measurement round-trip can happen on the Typst side instead of inside
// `runtime::measure`. They all run codegen under
// `ImportStrategy::EvalScope`, which omits `#set page` and the
// `/blockcell/lib.typ` import — the Typst-side `lib.typ` provides those
// via the `eval(..., scope: ...)` argument.
// =========================================================================

/// Plugin pass-1: build the probe-only Typst source, or `Ok(None)` if the
/// document has no measurement-aware diagrams (skip the round-trip).
pub fn emit_probes_for_plugin(source: &str) -> Result<Option<String>> {
    use crate::codegen::ImportStrategy;
    let doc = parse(source)?.document;
    let theme = Theme::default();
    Ok(crate::codegen::emit_probes(&doc, &theme, ImportStrategy::EvalScope)?.map(|(s, _ids)| s))
}

/// Plugin pass-2: build the final Typst source using `measurements`
/// collected on the Typst side (`query(<typstuml_measure>)`).
pub fn emit_layout_for_plugin(
    source: &str,
    measurements: &crate::runtime::MeasurementSet,
) -> Result<String> {
    use crate::codegen::ImportStrategy;
    let doc = parse(source)?.document;
    let theme = Theme::default();
    crate::codegen::emit(&doc, &theme, Some(measurements), ImportStrategy::EvalScope)
}

/// Plugin fast path for documents with no measurement-aware diagrams:
/// build the final Typst source directly, without expecting a measurement
/// round-trip.
pub fn emit_layout_no_measure(source: &str) -> Result<String> {
    use crate::codegen::ImportStrategy;
    let doc = parse(source)?.document;
    let theme = Theme::default();
    crate::codegen::emit(&doc, &theme, None, ImportStrategy::EvalScope)
}
