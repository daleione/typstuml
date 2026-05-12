//! IR → Typst code generation.
//!
//! The generated Typst is a small program that imports `blockcell` and
//! renders one diagram per page. The `blockcell` library is served from the
//! embedded virtual filesystem (`runtime::world`); from the Typst side it
//! looks like an ordinary file at `/blockcell/lib.typ`.

mod class;
mod json;
mod mindmap;
mod record_graph;
mod sequence;
mod tree_emit;
mod wbs;
mod yaml;

use crate::diagnostics::{Error, Result};
use crate::ir::{Diagram, Document};
use crate::runtime::MeasurementSet;
use crate::theme::Theme;

/// Render a [`Document`] to a self-contained Typst source string. When
/// `measurements` is `Some`, every measurement-aware codegen branch
/// (currently class) consumes the corresponding `mc-…` probe results;
/// missing IDs fall back to the Rust-side heuristic estimator silently.
pub fn emit(
    doc: &Document,
    theme: &Theme,
    measurements: Option<&MeasurementSet>,
) -> Result<String> {
    let mut out = String::new();

    write_preamble(&mut out, theme)?;

    for (idx, diagram) in doc.diagrams.iter().enumerate() {
        if idx > 0 {
            out.push_str("\n#pagebreak()\n\n");
        }
        match diagram {
            Diagram::Sequence(seq) => sequence::emit(&mut out, seq),
            Diagram::Json(j) => json::emit(&mut out, j, measurements, idx),
            Diagram::Yaml(y) => yaml::emit(&mut out, y, measurements, idx),
            Diagram::Wbs(w) => wbs::emit(&mut out, w),
            Diagram::MindMap(m) => mindmap::emit(&mut out, m),
            Diagram::Cuca(c) => class::emit(&mut out, c, measurements, idx),
        }
    }

    Ok(out)
}

/// Build the pass-1 (measure-only) Typst source. Returns the source
/// plus the list of probe IDs every consumer is expected to encounter
/// — used by [`crate::runtime::measure::run`] as a protocol-violation
/// guardrail. Returns `Ok(None)` when the document has no
/// measurement-aware diagrams (skip pass-1 entirely).
///
/// The pass-1 source must use the **same** theme preamble as
/// [`emit`]: text style flows through `measure()`, so any divergence
/// would mean the measured size doesn't match what pass-2 renders.
pub fn emit_probes(doc: &Document, theme: &Theme) -> Result<Option<(String, Vec<String>)>> {
    let any_probes = doc.diagrams.iter().any(|d| match d {
        Diagram::Cuca(c) => class::probe::has_probes(c),
        Diagram::Json(j) => record_graph::has_records(&j.root),
        Diagram::Yaml(y) => record_graph::has_records(&y.root),
        _ => false,
    });
    if !any_probes {
        return Ok(None);
    }

    let mut out = String::new();
    write_preamble(&mut out, theme)?;
    let mut expected_ids: Vec<String> = Vec::new();

    for (idx, diagram) in doc.diagrams.iter().enumerate() {
        match diagram {
            Diagram::Cuca(c) if class::probe::has_probes(c) => {
                class::probe::collect(c, idx, &mut out, &mut expected_ids);
            }
            Diagram::Json(j) if record_graph::has_records(&j.root) => {
                record_graph::collect_probes(&j.root, idx, &mut out, &mut expected_ids);
            }
            Diagram::Yaml(y) if record_graph::has_records(&y.root) => {
                record_graph::collect_probes(&y.root, idx, &mut out, &mut expected_ids);
            }
            _ => {}
        }
    }

    Ok(Some((out, expected_ids)))
}

/// Shared preamble: page setup + import + optional user preamble.
/// Kept byte-equivalent between pass-1 (measure) and pass-2 (render) so
/// `measure()` reads the same text style chain in both passes.
fn write_preamble(out: &mut String, theme: &Theme) -> Result<()> {
    out.push_str(
        "#set page(width: auto, height: auto, margin: 8pt)\n\
         #set text(size: 10pt)\n\
         #import \"/blockcell/lib.typ\": *\n\n",
    );

    if let Some(tpl_path) = &theme.preamble {
        let content = std::fs::read_to_string(tpl_path).map_err(|e| Error::Io {
            path: tpl_path.clone(),
            source: e,
        })?;
        out.push_str("// --- user preamble ---\n");
        out.push_str(&content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("// --- end user preamble ---\n\n");
    }
    Ok(())
}
