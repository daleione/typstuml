//! Hand-written PlantUML parser.
//!
//! Layout matches design doc §6.1:
//!
//! ```text
//! source text
//!   -> lexer / line scanner
//!   -> preprocessor
//!   -> diagram dispatcher
//!   -> per-diagram parser
//!   -> diagram-specific AST -> normalized IR
//! ```
//!
//! In M0 the per-diagram parsers are stubs that capture each block's raw
//! body plus a few layout hints. The actual parsing work happens during
//! codegen, where Sequence diagrams are routed to `blockcell`'s existing
//! `seq-puml` function. M1 replaces this with native parsing + golden tests.

pub mod dispatcher;
pub mod lexer;
pub mod preprocessor;
pub mod sequence;

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::Document;

pub use preprocessor::Config;

/// Top-level parser entry point. Threads the preprocessor config through,
/// extracts blocks, and dispatches to per-diagram parsers.
pub fn parse(
    source: &str,
    compat: CompatMode,
    config: &Config,
) -> Result<(Document, Vec<Diagnostic>)> {
    let pre = preprocessor::run_with(source, compat, config)?;
    let blocks = lexer::extract_uml_blocks(&pre.text);
    let mut diagnostics = pre.diagnostics;
    let mut diagrams = Vec::new();

    for block in &blocks {
        let kind = dispatcher::detect(block);
        match kind {
            dispatcher::DiagramKind::Sequence => {
                let (diagram, mut diags) = sequence::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            other => {
                let detail = format!(
                    "diagram type {other:?} is not yet supported in M0; \
                     only Sequence diagrams render today"
                );
                if compat == CompatMode::Strict {
                    return Err(Error::Unsupported {
                        kind: "diagram type",
                        detail,
                    });
                }
                diagnostics.push(Diagnostic {
                    level: Level::Warning,
                    line: Some(block.start_line),
                    message: detail,
                });
            }
        }
    }

    Ok((Document { diagrams }, diagnostics))
}
