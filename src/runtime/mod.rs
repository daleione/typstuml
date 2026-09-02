//! Typst runtime — a `typst-as-library` style world implementation plus a
//! render API that returns SVG / PDF / PNG bytes.
//!
//! The world serves three kinds of files:
//!   1. The main source (the Typst program emitted by `codegen`).
//!   2. Vendored `blockcell` sources, embedded at compile time via
//!      `include_dir!`. Visible to Typst as `/blockcell/lib.typ`, etc.
//!   3. Real on-disk files under the user's project root (used when the
//!      Typst program references local images / fonts via relative paths).
//!
//! Typst package downloads (`@preview/...`) are intentionally NOT supported —
//! the binary is fully offline. Add a downloader (or accept a pre-populated
//! cache) later if user templates need third-party packages.

pub mod measure;
#[cfg(feature = "embed-typst")]
mod world;

#[cfg(feature = "embed-typst")]
use std::path::PathBuf;

#[cfg(feature = "embed-typst")]
use typst::diag::{Severity, SourceDiagnostic};
#[cfg(feature = "embed-typst")]
use typst::ecow::EcoVec;
#[cfg(feature = "embed-typst")]
use typst::utils::Scalar;
#[cfg(feature = "embed-typst")]
use typst::WorldExt;
#[cfg(feature = "embed-typst")]
use typst_layout::PagedDocument;

#[cfg(feature = "embed-typst")]
use crate::diagnostics::{Diagnostic, Error, Level, Result};

pub use measure::{Measurement, MeasurementSet};
#[cfg(feature = "embed-typst")]
pub use world::TypstWorld;

// Runtime font injection — wasm-only because the native build already pulls
// fonts off the user's filesystem via `typst-kit`. See [`world::add_font`].
#[cfg(all(target_arch = "wasm32", feature = "embed-typst"))]
pub use world::add_font;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Format {
    Svg,
    Pdf,
    /// PNG raster output. `scale` is pixels per typographic point — the
    /// argument typst's renderer takes directly. 2.0 (= 144 DPI) is the
    /// sweet spot for retina screens and matches the historical default;
    /// the playground exposes 1×/2×/3×/4× to the user.
    Png {
        scale: f32,
    },
}

/// Default pixels-per-pt for PNG rendering: ~144 DPI. Path-based callers
/// (the CLI today, when no `--png-scale` flag exists yet) infer this.
pub const DEFAULT_PNG_SCALE: f32 = 2.0;

impl Format {
    pub fn infer_from_path(path: &std::path::Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("svg") => Some(Self::Svg),
            Some("pdf") => Some(Self::Pdf),
            Some("png") => Some(Self::Png {
                scale: DEFAULT_PNG_SCALE,
            }),
            _ => None,
        }
    }
}

// Everything below uses `typst::compile` and the SVG / PDF / PNG backends.
// Behind the `embed-typst` feature so the `typstuml-plugin` build (which
// runs inside an existing Typst process — Typst is the renderer there)
// stays free of these crates.

/// Natural page dimensions in typographic points.
#[cfg(feature = "embed-typst")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSize {
    pub width_pt: f32,
    pub height_pt: f32,
}

/// Outcome of a [`render`] call.
#[cfg(feature = "embed-typst")]
#[derive(Clone, Debug)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    /// Structured warnings collected during compilation and encoding. The CLI
    /// decides whether and where to display them.
    pub warnings: Vec<Diagnostic>,
    /// Natural size when the result contains exactly one page.
    pub size: Option<RenderSize>,
    /// Natural size of every page, in document order.
    pub page_sizes: Vec<RenderSize>,
}

/// Render `typst_source` to `format` and return the encoded bytes plus any
/// warnings produced during Typst compilation.
///
/// `root` is the project root used to resolve local `#image()` / `read()`
/// calls in user templates. `None` disables real-filesystem access entirely.
#[cfg(feature = "embed-typst")]
pub fn render(typst_source: String, root: Option<PathBuf>, format: Format) -> Result<Rendered> {
    let world = TypstWorld::new(root, typst_source);

    let warned = typst::compile::<PagedDocument>(&world);
    let mut warnings = lift_diagnostics(&world, &warned.warnings);
    let document = warned
        .output
        .map_err(|errors| typst_compile_error(&world, &errors))?;

    let page_sizes = document
        .pages()
        .iter()
        .map(|page| {
            let size = page.frame.size();
            RenderSize {
                width_pt: size.x.to_pt() as f32,
                height_pt: size.y.to_pt() as f32,
            }
        })
        .collect::<Vec<_>>();
    let size = (page_sizes.len() == 1).then(|| page_sizes[0]);

    let bytes = match format {
        Format::Svg => typst_svg::svg_merged(
            &document,
            &typst_svg::SvgOptions::default(),
            typst::layout::Abs::pt(2.0),
        )
        .into_bytes(),
        Format::Pdf => typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
            .map_err(|errors| typst_compile_error(&world, &errors))?,
        Format::Png { scale } => render_png(&document, scale, &mut warnings)?,
    };

    Ok(Rendered {
        bytes,
        warnings,
        size,
        page_sizes,
    })
}

#[cfg(feature = "embed-typst")]
fn render_png(
    document: &PagedDocument,
    scale: f32,
    warnings: &mut Vec<Diagnostic>,
) -> Result<Vec<u8>> {
    let pages = document.pages();
    let first = pages
        .first()
        .ok_or_else(|| render_error("document has no pages"))?;
    if pages.len() > 1 {
        warnings.push(Diagnostic::warning(format!(
            "PNG output only renders the first of {} pages; use SVG or PDF for multi-diagram inputs",
            pages.len()
        )));
    }
    // Clamp to a sane range: <0.5 is illegible; >16 is multi-GB-pixmap
    // territory and just wastes memory before typst-render OOMs.
    if !scale.is_finite() {
        return Err(render_error("PNG scale must be a finite number"));
    }
    let options = typst_render::RenderOptions {
        pixel_per_pt: Scalar::new(f64::from(scale.clamp(0.5, 16.0))),
        render_bleed: false,
    };
    let pixmap = typst_render::render(first, &options);
    pixmap
        .encode_png()
        .map_err(|e| render_error(format!("PNG encode failed: {e}")))
}

#[cfg(feature = "embed-typst")]
fn lift_diagnostics<W: typst::World>(
    world: &W,
    diags: &EcoVec<SourceDiagnostic>,
) -> Vec<Diagnostic> {
    diags
        .iter()
        .map(|d| {
            let (path, line, column) = diagnostic_location(world, d.span);
            Diagnostic {
                level: match d.severity {
                    Severity::Warning => Level::Warning,
                    Severity::Error => Level::Error,
                },
                path,
                line,
                column,
                message: d.message.to_string(),
                hints: d.hints.iter().map(|hint| hint.v.to_string()).collect(),
            }
        })
        .collect()
}

#[cfg(feature = "embed-typst")]
fn diagnostic_location<W: typst::World>(
    world: &W,
    span: typst::syntax::DiagSpan,
) -> (Option<String>, Option<usize>, Option<usize>) {
    let Some(id) = span.id() else {
        return (None, None, None);
    };
    let path = Some(id.vpath().get_with_slash().to_string());
    let Some(source) = world.source(id).ok() else {
        return (path, None, None);
    };
    let Some(range) = world.range(span) else {
        return (path, None, None);
    };
    let line = source
        .lines()
        .byte_to_line(range.start)
        .map(|line| line + 1);
    let column = source
        .lines()
        .byte_to_column(range.start)
        .map(|column| column + 1);
    (path, line, column)
}

#[cfg(feature = "embed-typst")]
pub(crate) fn format_typst_diagnostics<W: typst::World>(
    world: &W,
    errors: &EcoVec<SourceDiagnostic>,
) -> String {
    let mut out = String::new();
    for diag in errors {
        if let Some(id) = diag.span.id() {
            if let Ok(source) = world.source(id) {
                if let Some(range) = world.range(diag.span) {
                    let lines = source.lines();
                    let line = lines.byte_to_line(range.start).map(|l| l + 1);
                    let col = lines.byte_to_column(range.start).map(|c| c + 1);
                    let path = id.vpath().get_with_slash();
                    if let (Some(l), Some(c)) = (line, col) {
                        out.push_str(&format!("{path}:{l}:{c}: "));
                    }
                }
            }
        }
        let sev = match diag.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        out.push_str(&format!("{sev}: {}\n", diag.message));
        for hint in &diag.hints {
            out.push_str(&format!("  hint: {}\n", hint.v));
        }
    }
    if out.is_empty() {
        out.push_str("(no further detail)");
    }
    out
}

#[cfg(feature = "embed-typst")]
pub(crate) fn typst_compile_error<W: typst::World>(
    world: &W,
    errors: &EcoVec<SourceDiagnostic>,
) -> Error {
    Error::TypstCompile {
        message: format_typst_diagnostics(world, errors),
        diagnostics: lift_diagnostics(world, errors),
    }
}

#[cfg(feature = "embed-typst")]
fn render_error(message: impl Into<String>) -> Error {
    let message = message.into();
    Error::TypstCompile {
        diagnostics: vec![Diagnostic::error(message.clone())],
        message,
    }
}
