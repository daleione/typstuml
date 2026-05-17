//! IR → Typst code generation.
//!
//! The generated Typst is a small program that imports `blockcell` and
//! renders one diagram per page. Two emission targets:
//!
//! - [`ImportStrategy::VirtualFs`] (CLI / WASM playground): blockcell is
//!   served from the embedded virtual filesystem (`runtime::world`);
//!   preamble does `#import "/blockcell/lib.typ": *` and `#set page(...)`.
//!
//! - [`ImportStrategy::EvalScope`] (Typst plugin / `typstuml-plugin`): the
//!   emitted source is `eval()`-ed inside the host Typst document. The
//!   blockcell symbols are injected via the eval `scope:` argument by the
//!   Typst-side `lib.typ`, so we emit no `#import` here. We also skip the
//!   page-level `#set page` (we're embedded, not the document) and use
//!   parbreaks between diagrams instead of `#pagebreak()`.

mod activity;
mod cuca;
mod json;
mod mindmap;
mod record_graph;
mod sequence;
mod state;
mod tree_emit;
mod wbs;
mod yaml;

use crate::diagnostics::{Error, Result};
use crate::ir::{Diagram, Document};
use crate::runtime::MeasurementSet;
use crate::theme::Theme;

/// Which host the generated Typst source is destined for.
///
/// See module docs for the per-variant semantics. Default is `VirtualFs`
/// so existing call sites that pre-date the plugin form keep their
/// current behaviour byte-for-byte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportStrategy {
    /// CLI / WASM playground: full-document preamble, `#import` from the
    /// virtual `/blockcell/lib.typ`, `#pagebreak()` between diagrams.
    #[default]
    VirtualFs,
    /// Typst plugin: minimal preamble (no `#set page`, no `#import`),
    /// blockcell comes from the eval `scope:`, parbreaks between diagrams.
    EvalScope,
}

/// Every blockcell symbol the emitted Typst code references — both
/// top-level painters and the secondary symbols that appear as
/// arguments inside them (e.g. `swimlane(process[...], decision[...])`).
/// Mirrors the `#import` list in the slim `lib.typ` that `build.rs`
/// stages under `$OUT_DIR/blockcell/lib.typ`; the plugin path needs the
/// full set in its `eval(..., scope: ...)` argument so eval'd code can
/// resolve every name. Keep in sync with `build.rs::STAGED_LIB_TYP`.
pub const REFERENCED_BLOCKCELL_SYMBOLS: &[&str] = &[
    // records (record-graph painters + record-probe pass-1)
    "record-layout",
    "record-probe",
    // sequence
    "seq-puml",
    // tree / wbs / mindmap
    "tree",
    "node",
    "mindmap",
    // cuca (class / use-case / component painters + their probes)
    "cuca-layout",
    "cuca-probe",
    "container-probe",
    // state diagrams
    "state-layout",
    "state-probe",
    "state-note-probe",
    // activity atoms used as args inside the activity painters
    "process",
    "decision",
    "terminal",
    "junction",
    "edge",
    "flow-node",
    // activity composites / containers
    "flow-col",
    "section",
    // activity flow constructors / decorators
    "branch",
    "branch-merge",
    "switch",
    "case",
    "n-way",
    "fork-bar",
    "flow-loop",
    "start-marker",
    "stop-marker",
    "end-marker",
    "detach-marker",
    "partition",
    "flow-note",
    "with-notes",
    "swimlane",
    "lane",
    "swimlane-layout",
    "swimlane-probe",
];

/// Render a [`Document`] to a self-contained Typst source string. When
/// `measurements` is `Some`, every measurement-aware codegen branch
/// (currently class) consumes the corresponding `mc-…` probe results;
/// missing IDs fall back to the Rust-side heuristic estimator silently.
pub fn emit(
    doc: &Document,
    theme: &Theme,
    measurements: Option<&MeasurementSet>,
    imports: ImportStrategy,
) -> Result<String> {
    let mut out = String::new();

    write_preamble(&mut out, theme, imports)?;

    let separator = match imports {
        ImportStrategy::VirtualFs => "\n#pagebreak()\n\n",
        ImportStrategy::EvalScope => "\n\n",
    };

    for (idx, diagram) in doc.diagrams.iter().enumerate() {
        if idx > 0 {
            out.push_str(separator);
        }
        match diagram {
            Diagram::Sequence(seq) => sequence::emit(&mut out, seq),
            Diagram::Json(j) => json::emit(&mut out, j, measurements, idx),
            Diagram::Yaml(y) => yaml::emit(&mut out, y, measurements, idx),
            Diagram::Wbs(w) => wbs::emit(&mut out, w),
            Diagram::MindMap(m) => mindmap::emit(&mut out, m),
            Diagram::Cuca(c) => cuca::emit(&mut out, c, measurements, idx),
            Diagram::Activity(a) => activity::emit(&mut out, a, measurements, idx),
            Diagram::State(s) => state::emit(&mut out, s, measurements, idx),
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
/// The pass-1 source must use the **same** preamble as [`emit`] under
/// the same [`ImportStrategy`]: text style flows through `measure()`,
/// so any divergence would mean the measured size doesn't match what
/// pass-2 renders.
pub fn emit_probes(
    doc: &Document,
    theme: &Theme,
    imports: ImportStrategy,
) -> Result<Option<(String, Vec<String>)>> {
    let any_probes = doc.diagrams.iter().any(|d| match d {
        Diagram::Cuca(c) => cuca::probe::has_probes(c),
        Diagram::Json(j) => record_graph::has_records(&j.root),
        Diagram::Yaml(y) => record_graph::has_records(&y.root),
        Diagram::State(s) => state::has_probes(s),
        // Activity needs probes only for the swimlane layout pipeline;
        // ordinary flow-col activities self-measure on the Typst side.
        Diagram::Activity(a) => activity::has_swimlane_probes(a),
        _ => false,
    });
    if !any_probes {
        return Ok(None);
    }

    let mut out = String::new();
    write_preamble(&mut out, theme, imports)?;
    let mut expected_ids: Vec<String> = Vec::new();

    for (idx, diagram) in doc.diagrams.iter().enumerate() {
        match diagram {
            Diagram::Cuca(c) if cuca::probe::has_probes(c) => {
                cuca::probe::collect(c, idx, &mut out, &mut expected_ids);
            }
            Diagram::Json(j) if record_graph::has_records(&j.root) => {
                record_graph::collect_probes(&j.root, idx, &mut out, &mut expected_ids);
            }
            Diagram::Yaml(y) if record_graph::has_records(&y.root) => {
                record_graph::collect_probes(&y.root, idx, &mut out, &mut expected_ids);
            }
            Diagram::State(s) if state::has_probes(s) => {
                state::collect_probes(s, idx, &mut out, &mut expected_ids);
            }
            Diagram::Activity(a) if activity::has_swimlane_probes(a) => {
                activity::collect_probes(a, idx, &mut out, &mut expected_ids);
            }
            _ => {}
        }
    }

    Ok(Some((out, expected_ids)))
}

/// Shared preamble. Two shapes per [`ImportStrategy`] — see module docs.
fn write_preamble(out: &mut String, theme: &Theme, imports: ImportStrategy) -> Result<()> {
    match imports {
        ImportStrategy::VirtualFs => {
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
        }
        ImportStrategy::EvalScope => {
            // The host document owns page setup; blockcell symbols are
            // injected via the eval `scope:` argument. We still pin
            // text size so pass-1 measurements line up with pass-2
            // layout numbers (which were tuned for 10pt). The pin is
            // scoped to the eval, doesn't leak to the host document.
            out.push_str("#set text(size: 10pt)\n\n");
            // theme.preamble is intentionally not applied in plugin
            // form for v1 — the host document is the place for that.
            let _ = theme;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::REFERENCED_BLOCKCELL_SYMBOLS;
    use std::collections::HashSet;

    /// Pulled from the staged `$OUT_DIR/blockcell/lib.typ` that `build.rs`
    /// writes from `STAGED_LIB_TYP`. Parsing the file the build actually
    /// emits avoids duplicating the import list across build.rs and the
    /// test, and means this fires the moment build.rs's `STAGED_LIB_TYP`
    /// stops re-exporting a symbol that codegen still emits.
    const STAGED_LIB_TYP: &str =
        include_str!(concat!(env!("OUT_DIR"), "/blockcell/lib.typ"));

    /// Extract every symbol exported by an `#import "src/...": a, b, c`
    /// line. Tolerates whitespace and comments; ignores everything that
    /// isn't an import line.
    fn imported_symbols(src: &str) -> HashSet<String> {
        let mut out: HashSet<String> = HashSet::new();
        for line in src.lines() {
            let line = line.trim();
            // We only export via `#import "..": a, b, c` — colon split
            // gives us the symbol list on the right.
            let Some(rest) = line.strip_prefix("#import") else { continue };
            let Some((_, names)) = rest.split_once(':') else { continue };
            for name in names.split(',') {
                let name = name.trim();
                if !name.is_empty() {
                    out.insert(name.to_string());
                }
            }
        }
        out
    }

    /// Every symbol codegen could emit (`REFERENCED_BLOCKCELL_SYMBOLS`)
    /// must be exported by the staged `lib.typ`. If this fails the
    /// plugin path will blow up at eval time with "unknown identifier"
    /// even though the CLI (which imports `*`) keeps working.
    #[test]
    fn referenced_symbols_are_exported_by_staged_lib() {
        let exported = imported_symbols(STAGED_LIB_TYP);
        let missing: Vec<&&str> = REFERENCED_BLOCKCELL_SYMBOLS
            .iter()
            .filter(|s| !exported.contains(**s))
            .collect();
        assert!(
            missing.is_empty(),
            "REFERENCED_BLOCKCELL_SYMBOLS contains symbols not exported by \
             STAGED_LIB_TYP in build.rs: {missing:?}. Add them to the \
             matching `#import \"src/...\": ...` line."
        );
    }
}
