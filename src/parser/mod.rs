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
//! Sequence, JSON, YAML, WBS, and mind-map diagrams have native parsers.
//! Other diagram types are recognized by the dispatcher but don't have
//! parsers yet — `--compat strict` rejects them, otherwise we emit a
//! warning and skip.
//!
//! [`StructuredSequence`]: crate::ir::StructuredSequence

pub mod cuca;
pub mod dispatcher;
pub mod json;
pub mod lexer;
pub mod mindmap;
pub mod preprocessor;
pub mod sequence;
pub mod wbs;
pub mod yaml;

use std::path::PathBuf;

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::Document;

pub use preprocessor::Config;

/// Result of a top-level parse. `includes` is the canonical-path list of
/// every file pulled in by `!include`, used by `watch` mode to subscribe to
/// include-side changes.
#[derive(Debug)]
pub struct ParseOutput {
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
    pub includes: Vec<PathBuf>,
}

/// Top-level parser entry point. Threads the preprocessor config through,
/// extracts blocks, and dispatches to per-diagram parsers.
pub fn parse(source: &str, compat: CompatMode, config: &Config) -> Result<ParseOutput> {
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
            dispatcher::DiagramKind::Json => {
                let (diagram, mut diags) = json::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            dispatcher::DiagramKind::Yaml => {
                let (diagram, mut diags) = yaml::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            dispatcher::DiagramKind::Wbs => {
                let (diagram, mut diags) = wbs::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            dispatcher::DiagramKind::MindMap => {
                let (diagram, mut diags) = mindmap::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            dispatcher::DiagramKind::Cuca => {
                let (diagram, mut diags) = cuca::parse(block, compat)?;
                diagrams.push(diagram);
                diagnostics.append(&mut diags);
            }
            other => {
                let detail = format!(
                    "diagram type {other:?} is not yet supported; \
                     Sequence, JSON, YAML, WBS, and mind-map diagrams render today"
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

    Ok(ParseOutput {
        document: Document { diagrams },
        diagnostics,
        includes: pre.includes,
    })
}
