//! TypstUML — render PlantUML diagrams via Typst.
//!
//! This crate is organized as a single crate with module-level layering, per
//! the project's architecture decision (see the design doc in the parent
//! `blockcell` repo). Crate splits are deferred until M3+ when boundaries
//! settle.
//!
//! Pipeline: source text → [`parser`] → [`ir`] → [`codegen`] → [`runtime`].
//! [`theme`] feeds into codegen; [`cli`] orchestrates the whole thing.

pub mod cli;
pub mod codegen;
pub mod diagnostics;
pub mod ir;
pub mod parser;
pub mod runtime;
pub mod theme;
