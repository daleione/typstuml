//! Host-driven two-pass codegen. Versioned wire decoding belongs to the plugin crate.
use super::{parse_with_options, RenderOptions};
use crate::{diagnostics::Result, parser::InputLanguage, theme::Theme};

/// Plugin pass-1: build the probe-only Typst source, or `Ok(None)` if the
/// document has no measurement-aware diagrams (skip the round-trip).
pub fn emit_probes_for_plugin(source: &str) -> Result<Option<String>> {
    use crate::codegen::ImportStrategy;
    let doc = parse_with_options(source, RenderOptions::default())?.document;
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
    let doc = parse_with_options(source, RenderOptions::default())?.document;
    let theme = Theme::default();
    crate::codegen::emit(&doc, &theme, Some(measurements), ImportStrategy::EvalScope)
}

/// Plugin fast path for documents with no measurement-aware diagrams:
/// build the final Typst source directly, without expecting a measurement
/// round-trip.
pub fn emit_layout_no_measure(source: &str) -> Result<String> {
    use crate::codegen::ImportStrategy;
    let doc = parse_with_options(source, RenderOptions::default())?.document;
    let theme = Theme::default();
    crate::codegen::emit(&doc, &theme, None, ImportStrategy::EvalScope)
}

/// Versioned probe response for hosts that perform the measurement pass.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProbeBundle {
    pub source: String,
    pub expected_ids: Vec<String>,
}
pub fn emit_probe_bundle(source: &str, language: InputLanguage) -> Result<ProbeBundle> {
    let options = RenderOptions::for_plugin(language);
    let doc = parse_with_options(source, options)?.document;
    let (source, expected_ids) =
        crate::codegen::emit_probes_with_options(&doc, &Theme::default(), options.codegen)?
            .unwrap_or_default();
    crate::runtime::MeasurementSet::validate_expected_ids(&expected_ids)?;
    Ok(ProbeBundle {
        source,
        expected_ids,
    })
}
pub fn emit_layout_with_language(
    source: &str,
    language: InputLanguage,
    measurements: Option<&crate::runtime::MeasurementSet>,
) -> Result<String> {
    let options = RenderOptions::for_plugin(language);
    let doc = parse_with_options(source, options)?.document;
    let theme = Theme::default();
    if let Some(set) = measurements {
        let (_, ids) = crate::codegen::emit_probes_with_options(&doc, &theme, options.codegen)?
            .unwrap_or_default();
        set.validate(&ids)?;
    }
    crate::codegen::emit_with_options(&doc, &theme, measurements, options.codegen)
}
