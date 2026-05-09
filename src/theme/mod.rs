//! Theme / `skinparam` engine.
//!
//! Per design doc §8 the theme system has three layers: raw PlantUML
//! parameters, a normalized internal `Theme` value, and the Typst preamble
//! codegen emits to apply it. The current `Theme` only carries an optional
//! user-supplied Typst preamble; `skinparam` extraction is done by the
//! sequence parser into [`crate::ir::Skinparam`] and codegen translates a
//! small high-frequency subset into a Typst preamble.

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Theme {
    /// User-supplied Typst preamble injected before the diagram body.
    pub preamble: Option<PathBuf>,
}
