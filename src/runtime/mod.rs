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
mod world;

use std::path::PathBuf;

use typst::diag::{Severity, SourceDiagnostic};
use typst::ecow::EcoVec;

use crate::diagnostics::{Diagnostic, Error, Level, Result};

pub use measure::{Measurement, MeasurementSet};
pub use world::TypstWorld;

// Runtime font injection — wasm-only because the native build already pulls
// fonts off the user's filesystem via `typst-kit`. See [`world::add_font`].
#[cfg(target_arch = "wasm32")]
pub use world::add_font;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Format {
    Svg,
    Pdf,
    /// PNG raster output. `scale` is pixels per typographic point — the
    /// argument typst's renderer takes directly. 2.0 (= 144 DPI) is the
    /// sweet spot for retina screens and matches the historical default;
    /// the playground exposes 1×/2×/3×/4× to the user.
    Png { scale: f32 },
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
            Some("png") => Some(Self::Png { scale: DEFAULT_PNG_SCALE }),
            _ => None,
        }
    }
}

/// Outcome of a [`render`] call.
pub struct Rendered {
    pub bytes: Vec<u8>,
    /// Typst-side warnings collected during compilation. The CLI surfaces
    /// these on stderr.
    pub warnings: Vec<Diagnostic>,
}

/// Render `typst_source` to `format` and return the encoded bytes plus any
/// warnings produced during Typst compilation.
///
/// `root` is the project root used to resolve local `#image()` / `read()`
/// calls in user templates. Pass `None` to use the current working dir.
pub fn render(typst_source: String, root: Option<PathBuf>, format: Format) -> Result<Rendered> {
    let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let world = TypstWorld::new(root, typst_source);

    let warned = typst::compile(&world);
    let warnings = lift_diagnostics(&world, &warned.warnings);
    let document = warned
        .output
        .map_err(|errors| Error::TypstCompile(format_typst_diagnostics(&world, &errors)))?;

    let bytes = match format {
        Format::Svg => typst_svg::svg_merged(&document, typst::layout::Abs::pt(2.0)).into_bytes(),
        Format::Pdf => typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
            .map_err(|errors| Error::TypstCompile(format_typst_diagnostics(&world, &errors)))?,
        Format::Png { scale } => render_png(&document, scale)?,
    };

    Ok(Rendered { bytes, warnings })
}

fn render_png(document: &typst::layout::PagedDocument, scale: f32) -> Result<Vec<u8>> {
    let pages = &document.pages;
    let first = pages
        .first()
        .ok_or_else(|| Error::TypstCompile("document has no pages".to_string()))?;
    if pages.len() > 1 {
        eprintln!(
            "typstuml: warning: PNG output only renders the first of {} pages; \
             use SVG or PDF for multi-diagram inputs",
            pages.len()
        );
    }
    // Clamp to a sane range: <0.5 is illegible; >16 is multi-GB-pixmap
    // territory and just wastes memory before typst-render OOMs.
    let scale = scale.clamp(0.5, 16.0);
    let pixmap = typst_render::render(first, scale);
    pixmap
        .encode_png()
        .map_err(|e| Error::TypstCompile(format!("PNG encode failed: {e}")))
}

fn lift_diagnostics<W: typst::World>(
    world: &W,
    diags: &EcoVec<SourceDiagnostic>,
) -> Vec<Diagnostic> {
    diags
        .iter()
        .map(|d| Diagnostic {
            level: match d.severity {
                Severity::Warning => Level::Warning,
                Severity::Error => Level::Error,
            },
            line: span_line(world, d.span),
            message: d.message.to_string(),
        })
        .collect()
}

fn span_line<W: typst::World>(world: &W, span: typst::syntax::Span) -> Option<usize> {
    let id = span.id()?;
    let source = world.source(id).ok()?;
    let range = source.range(span)?;
    Some(source.lines().byte_to_line(range.start)? + 1)
}

pub(crate) fn format_typst_diagnostics<W: typst::World>(
    world: &W,
    errors: &EcoVec<SourceDiagnostic>,
) -> String {
    let mut out = String::new();
    for diag in errors {
        if let Some(id) = diag.span.id() {
            if let Ok(source) = world.source(id) {
                if let Some(range) = source.range(diag.span) {
                    let lines = source.lines();
                    let line = lines.byte_to_line(range.start).map(|l| l + 1);
                    let col = lines.byte_to_column(range.start).map(|c| c + 1);
                    let path = id.vpath().as_rooted_path().display();
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
            out.push_str(&format!("  hint: {hint}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("(no further detail)");
    }
    out
}
