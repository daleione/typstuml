//! TypstUML — render PlantUML diagrams via Typst.
//!
//! Module-level layering inside one library crate. The only sibling crate in
//! the workspace is `typstuml-wasm`, a thin `wasm-bindgen` binding layer over
//! [`render`]; further crate splits are deferred until module boundaries settle.
//!
//! Pipeline: source text → [`parser`] → [`ir`] → [`codegen`] → [`runtime`].
//! [`theme`] feeds into codegen. [`cli`] orchestrates the whole thing on
//! native targets; [`render`] is the slim, filesystem-free entry point that
//! wasm builds and other embedders call into.

pub mod codegen;
pub mod diagnostics;
pub mod ir;
pub mod layout;
pub mod parser;
pub mod render;
pub mod runtime;
pub mod theme;
pub mod web;

// The CLI pulls in clap, file-watching, and stdio — none of which exist on
// wasm32. Embedders there go through [`render`] instead. It also depends
// on the full Typst-as-library runtime, so it requires `embed-typst`.
#[cfg(all(not(target_arch = "wasm32"), feature = "embed-typst"))]
pub mod cli;
