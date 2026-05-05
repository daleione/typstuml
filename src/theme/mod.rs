//! Theme / `skinparam` engine.
//!
//! Per design doc §8 the theme system has three layers: raw PlantUML
//! parameters, a normalized internal `Theme` value, and the Typst preamble
//! codegen emits to apply it. M0 only carries the user's `--theme` choice
//! and an optional Typst-template path; the actual `skinparam` extraction
//! lands in M1.

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Theme {
    /// Built-in theme name (e.g. `"plain"`, `"vibrant"`). Reserved for M1.
    pub name: Option<String>,
    /// User-supplied Typst preamble injected before the diagram body.
    pub typst_template: Option<PathBuf>,
}
