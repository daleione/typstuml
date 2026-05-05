//! Theme / `skinparam` engine.
//!
//! Per design doc §8 the theme system has three layers: raw PlantUML
//! parameters, a normalized internal `Theme` value, and the Typst preamble
//! codegen emits to apply it. The current `Theme` only carries the user's
//! `--theme` choice and an optional Typst-template path; `skinparam`
//! extraction is done by the sequence parser into [`crate::ir::Skinparam`]
//! and codegen translates a small high-frequency subset into a Typst
//! preamble.

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Theme {
    /// Built-in theme name (e.g. `"plain"`, `"vibrant"`). Not yet wired.
    pub name: Option<String>,
    /// User-supplied Typst preamble injected before the diagram body.
    pub typst_template: Option<PathBuf>,
}
