//! TypstUML — render PlantUML diagrams via Typst.
//!
//! Single crate, module-level layering. Crate splits are deferred until
//! module boundaries settle.
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
