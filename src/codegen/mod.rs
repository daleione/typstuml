//! IR → Typst code generation.
//!
//! The generated Typst is a small program that imports `blockcell` and
//! renders one diagram per page. The `blockcell` library is served from the
//! embedded virtual filesystem (`runtime::world`); from the Typst side it
//! looks like an ordinary file at `/blockcell/lib.typ`.

mod json;
mod sequence;

use crate::diagnostics::{Error, Result};
use crate::ir::{Diagram, Document};
use crate::theme::Theme;

/// Render a [`Document`] to a self-contained Typst source string.
pub fn emit(doc: &Document, theme: &Theme) -> Result<String> {
    let mut out = String::new();

    out.push_str(
        "#set page(width: auto, height: auto, margin: 8pt)\n\
         #set text(size: 10pt)\n\
         #import \"/blockcell/lib.typ\": *\n\n",
    );

    if let Some(tpl_path) = &theme.typst_template {
        let content = std::fs::read_to_string(tpl_path).map_err(|e| Error::Io {
            path: tpl_path.clone(),
            source: e,
        })?;
        out.push_str("// --- user template ---\n");
        out.push_str(&content);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("// --- end user template ---\n\n");
    }

    for (idx, diagram) in doc.diagrams.iter().enumerate() {
        if idx > 0 {
            out.push_str("\n#pagebreak()\n\n");
        }
        match diagram {
            Diagram::Sequence(seq) => sequence::emit(&mut out, seq),
            Diagram::Json(j) => json::emit(&mut out, j),
        }
    }

    Ok(out)
}
