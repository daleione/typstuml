//! Filesystem-free rendering API. CLI and embedders share the measurement pipeline;
//! hosts retain ownership of includes, file I/O, and diagnostics presentation.
mod options;
#[cfg(feature = "embed-typst")]
pub(crate) mod pipeline;
mod plugin;
use crate::diagnostics::{Error, Result};
#[cfg(feature = "embed-typst")]
use crate::parser::InputLanguage;
#[cfg(feature = "embed-typst")]
pub use crate::runtime::{Format, RenderSize, Rendered};
#[cfg(feature = "embed-typst")]
use crate::{runtime, theme::Theme};
pub use options::{MeasurementPolicy, RenderOptions};
pub use plugin::{
    emit_layout_for_plugin, emit_layout_no_measure, emit_layout_with_language, emit_probe_bundle,
    emit_probes_for_plugin, ProbeBundle,
};

#[cfg(feature = "embed-typst")]
pub fn render_source(source: &str, format: Format) -> Result<Rendered> {
    render_source_with_options(source, RenderOptions::default(), format)
}
#[cfg(feature = "embed-typst")]
pub fn render_source_with_language(
    source: &str,
    language: InputLanguage,
    format: Format,
) -> Result<Rendered> {
    render_source_with_options(source, RenderOptions::for_language(language), format)
}
#[cfg(feature = "embed-typst")]
pub fn render_source_with_options(
    source: &str,
    options: RenderOptions,
    format: Format,
) -> Result<Rendered> {
    let parsed = parse_with_options(source, options)?;
    let prepared = pipeline::prepare(&parsed.document, &Theme::default(), None, options)?;
    let mut rendered = runtime::render(prepared.source, None, format)?;
    let mut warnings = parsed.diagnostics;
    warnings.extend(prepared.warnings);
    warnings.append(&mut rendered.warnings);
    rendered.warnings = warnings;
    Ok(rendered)
}
#[cfg(feature = "embed-typst")]
pub fn emit_typst(source: &str) -> Result<String> {
    emit_typst_with_options(source, RenderOptions::default())
}
#[cfg(feature = "embed-typst")]
pub fn emit_typst_with_language(source: &str, language: InputLanguage) -> Result<String> {
    emit_typst_with_options(source, RenderOptions::for_language(language))
}
#[cfg(feature = "embed-typst")]
pub fn emit_typst_with_options(source: &str, options: RenderOptions) -> Result<String> {
    let parsed = parse_with_options(source, options)?;
    pipeline::prepare(&parsed.document, &Theme::default(), None, options).map(|p| p.source)
}
fn parse_with_options(source: &str, options: RenderOptions) -> Result<crate::parser::ParseOutput> {
    let parsed = crate::parser::parse_with_language(
        source,
        options.language,
        options.compat,
        &crate::parser::Config::default(),
    )?;
    if parsed.document.diagrams.is_empty() {
        return Err(Error::Cli("no supported diagrams found in input".into()));
    }
    Ok(parsed)
}
